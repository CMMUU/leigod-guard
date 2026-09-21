#!/bin/sh
set -eu
umask 077
backup_dir=/opt/leigod-guard/backups
mkdir -p "$backup_dir"
backup_file="$backup_dir/guard-$(date -u +%Y%m%dT%H%M%SZ).dump"
trap 'rm -f "$backup_file.tmp"' EXIT
# The PostgreSQL service administrator reads only the dedicated project DB.
# Credentials are resolved inside the container and never enter arguments/logs.
docker exec postgresql sh -c 'exec pg_dump -U "$POSTGRES_USER" -d leigod_guard -Fc --no-owner --no-acl' > "$backup_file.tmp"
test -s "$backup_file.tmp"
docker exec -i postgresql pg_restore --list < "$backup_file.tmp" > /dev/null
mv "$backup_file.tmp" "$backup_file"
find "$backup_dir" -maxdepth 1 -type f -name 'guard-*.dump' -mtime +7 -delete
printf 'Leigod Guard backup verified and saved.\n'
# Installing backup-recipient.txt opts this deployment into encrypted offsite backup.
if [ -f /opt/leigod-guard/secrets/backup-recipient.txt ]; then
 python3 /opt/leigod-guard/bin/backup-offsite.py "$backup_file"
 find "$backup_dir" -maxdepth 1 -type f -name 'guard-*.tar.age' -mtime +7 -delete
fi
