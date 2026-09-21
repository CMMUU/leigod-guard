#!/usr/bin/env bash
# Disposable Linux Docker validation. No real email or Leigod credentials.
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
scratch=$(mktemp -d)
project="guard-selfhost-${GITHUB_RUN_ID:-local}-$$"
export GUARD_ENV_FILE="$scratch/server.env"
export GUARD_SECRET_FILE="$scratch/secrets/remote.key"
export GUARD_IMAGE=leigod-guard-server:smoke APP_PORT=13088
dc=(docker compose -p "$project" --env-file "$GUARD_ENV_FILE" -f "$repo/deploy/server/compose.yaml")
cleanup() {
    "${dc[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
    rm -rf "$scratch"
}
trap cleanup EXIT
python3 "$repo/deploy/server/init-config.py" --docker --output "$scratch"
if python3 "$repo/deploy/server/init-config.py" --docker --output "$scratch" >/dev/null 2>&1; then
    echo 'Initialization must refuse existing secrets' >&2
    exit 1
fi
sed -i 's|PUBLIC_ORIGIN=http://127.0.0.1:3088|PUBLIC_ORIGIN=http://127.0.0.1:13088|' "$GUARD_ENV_FILE"
"${dc[@]}" config --quiet
docker run --rm -e PUBLIC_ORIGIN=https://guard.example.invalid \
    -v "$repo/deploy/server/Caddyfile:/etc/caddy/Caddyfile:ro" \
    caddy:2-alpine caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
"${dc[@]}" up -d --no-build --wait --wait-timeout 120 app
curl -fsS http://127.0.0.1:13088/ | grep -q '<html'
curl -fsS http://127.0.0.1:13088/api/health | python3 -c 'import json,sys; h=json.load(sys.stdin); assert h["status"]=="ok" and h["remote_execution"] is False'
export ADMIN_USERNAME=smoke-admin ADMIN_PASSWORD=disposable-self-host-password
"${dc[@]}" run --rm -T -e ADMIN_USERNAME -e ADMIN_PASSWORD app create-admin
unset ADMIN_USERNAME ADMIN_PASSWORD
"${dc[@]}" exec -T app sh -c 'test "$(id -u)" = 10001 && test ! -w /app'
"${dc[@]}" exec -T postgres pg_dump -U guard -d leigod_guard -Fc --no-owner --no-acl > "$scratch/database.dump"
"${dc[@]}" exec -T postgres createdb -U guard restore_check
"${dc[@]}" exec -T postgres pg_restore -U guard -d restore_check --no-owner --no-acl < "$scratch/database.dump"
count=$("${dc[@]}" exec -T postgres psql -U guard -d restore_check -Atc 'SELECT count(*) FROM users')
test "$count" = 1
"${dc[@]}" down
sed -i 's/REMOTE_EXECUTION=false/REMOTE_EXECUTION=true/' "$GUARD_ENV_FILE"
"${dc[@]}" up -d --no-build --wait --wait-timeout 120 app
curl -fsS http://127.0.0.1:13088/api/health | python3 -c 'import json,sys; h=json.load(sys.stdin); assert h["remote_execution"] is True'
python3 - <<'PY'
import http.cookiejar, json, urllib.request
client=urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
request=urllib.request.Request('http://127.0.0.1:13088/api/login',
    data=json.dumps({'username':'smoke-admin','password':'disposable-self-host-password','remember':True}).encode(),
    headers={'Content-Type':'application/json','Origin':'http://127.0.0.1:13088'})
with client.open(request,timeout=10) as response:
    body=json.load(response)
    assert response.status==200 and body['ok'] is True
with client.open('http://127.0.0.1:13088/api/me',timeout=10) as response:
    assert json.load(response)['role']=='admin'
print('PASS: container networking, static assets, non-root, private key, administrator, persistence and backup restore')
PY
