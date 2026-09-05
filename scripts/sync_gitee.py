#!/usr/bin/env python3
"""One-way repository/release synchronization. No GitHub writes; no asset replacement."""
from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode, urljoin, urlsplit
from urllib.request import Request, HTTPRedirectHandler, build_opener
import uuid

REPOS = {"leigod-guard", "mihomo-codex"}
GH_OWNER, GE_OWNER = "CMMUU", "cmmuu"
GH_API, GE_API = "https://api.github.com", "https://gitee.com/api/v5"
MAX_JSON, MAX_ASSET = 8 * 1024 * 1024, 512 * 1024 * 1024
GH_STORAGE = {"release-assets.githubusercontent.com", "objects.githubusercontent.com", "github-releases.githubusercontent.com"}
GE_STORAGE = {"foruda.gitee.com"}


class SyncError(Exception):
    pass


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def safe_name(value):
    if (not isinstance(value, str) or not value or value in {".", ".."}
            or value.endswith((" ", ".")) or any(ord(c) < 32 or c in '/\\:<>"|?*' for c in value)):
        raise SyncError("Unsafe attachment filename")
    return value


def sha256(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def source_digest(asset):
    value = asset.get("digest")
    if value is None:
        return None
    if not isinstance(value, str) or not re.fullmatch(r"sha256:[0-9a-fA-F]{64}", value):
        raise SyncError("Unsupported GitHub asset digest")
    return value[7:].lower()


def validate_pair(repo, source, target):
    if repo not in REPOS:
        raise SyncError("Repository is outside the permitted migration scope")
    for info, owner in ((source, GH_OWNER), (target, GE_OWNER)):
        if (str(info.get("owner", {}).get("login", "")).casefold() != owner.casefold()
                or type(info.get("private")) is not bool):
            raise SyncError("Repository owner, name or visibility could not be confirmed")
    if str(source.get("full_name", "")).casefold() != f"{GH_OWNER}/{repo}".casefold():
        raise SyncError("GitHub source repository does not match the requested scope")
    if (str(target.get("path", "")).casefold() != repo.casefold()
            or str(target.get("html_url", "")).rstrip("/").casefold() != f"https://gitee.com/{GE_OWNER}/{repo}".casefold()):
        raise SyncError("Gitee target path does not match the requested scope")
    if source["private"] and not target["private"]:
        raise SyncError("Private GitHub source must never synchronize to a public Gitee target")


def checked_url(url, allowed_hosts):
    parsed = urlsplit(url)
    try:
        port = parsed.port
    except ValueError:
        raise SyncError("Invalid download port") from None
    if (parsed.scheme != "https" or parsed.hostname not in allowed_hosts or port not in (None, 443)
            or parsed.username or parsed.password or parsed.fragment):
        raise SyncError("Untrusted download redirect; no credentials were forwarded")
    return parsed


class Api:
    def __init__(self, service, token, storage_hosts=()):
        self.service, self.token = service, token
        self.base = GH_API if service == "github" else GE_API
        self.host = "api.github.com" if service == "github" else "gitee.com"
        self.storage_hosts = GH_STORAGE if service == "github" else GE_STORAGE | set(storage_hosts)
        self.opener = build_opener(NoRedirect())

    def headers(self):
        headers = {"Authorization": "Bearer " + self.token, "User-Agent": "CMMUU-Gitee-Sync/1", "Accept": "application/json"}
        if self.service == "github":
            headers["X-GitHub-Api-Version"] = "2022-11-28"
        return headers

    def request(self, path, method="GET", data=None):
        if self.service == "github" and method != "GET":
            raise SyncError("GitHub writes are not supported")
        if not path.startswith("/") or "access_token=" in path or "token=" in path:
            raise SyncError("Invalid API path")
        headers, body = self.headers(), None
        if data is not None:
            # Gitee documents access_token in formData for writes; keep it in
            # the in-memory body, never a URL, file, command argument or log.
            fields = {**data, "access_token": self.token}
            body = urlencode(fields).encode("utf-8")
            headers["Content-Type"] = "application/x-www-form-urlencoded"
        req = Request(self.base + path, data=body, headers=headers, method=method)
        try:
            with self.opener.open(req, timeout=45) as response:
                raw = response.read(MAX_JSON + 1)
                if len(raw) > MAX_JSON:
                    raise SyncError("API response exceeds the metadata limit")
                return json.loads(raw)
        except HTTPError as error:
            # Never print response bodies, full URLs or signed query strings.
            raise SyncError(f"{self.service} API returned HTTP {error.code}; no write was retried") from None
        except (URLError, TimeoutError, OSError, ValueError, http.client.HTTPException):
            raise SyncError(f"{self.service} API request failed; details suppressed to protect credentials") from None

    def pages(self, path):
        rows = []
        for page in range(1, 1001):
            data = self.request(path + ("&" if "?" in path else "?") + urlencode({"page": page, "per_page": 100}))
            if not isinstance(data, list):
                raise SyncError("Unexpected paginated API response")
            rows.extend(data)
            if len(data) < 100:
                return rows
        raise SyncError("Pagination limit exceeded")

    def download(self, path, destination, expected_size, expected_sha=None):
        if type(expected_size) is not int or not 0 <= expected_size <= MAX_ASSET:
            raise SyncError("Attachment size is outside the supported limit")
        destination = Path(destination)
        if destination.exists():
            if destination.is_symlink() or not destination.is_file() or destination.stat().st_size != expected_size:
                raise SyncError("Existing local attachment has a different size or type")
            actual = sha256(destination)
            if expected_sha is None:
                raise SyncError("An unhashed cached source cannot be trusted; use a fresh working directory")
            if actual == expected_sha:
                return actual
            raise SyncError("Existing local attachment has a different SHA-256; it was not overwritten")
        url, headers = self.base + path, self.headers()
        headers["Accept"] = "application/octet-stream"
        allowed = {self.host} | self.storage_hosts
        part = destination.with_name(destination.name + ".part-" + uuid.uuid4().hex)
        destination.parent.mkdir(parents=True, exist_ok=True)
        try:
            for redirect in range(6):
                checked_url(url, allowed)
                try:
                    response = self.opener.open(Request(url, headers=headers), timeout=90)
                    break
                except HTTPError as error:
                    if error.code not in (301, 302, 303, 307, 308) or redirect == 5:
                        raise SyncError(f"{self.service} attachment returned HTTP {error.code}") from None
                    location = error.headers.get("Location", "")
                    if not location or self.token in location:
                        raise SyncError("Missing redirect or a redirect containing an authentication credential")
                    location = urljoin(url, location)
                    checked_url(location, allowed)
                    # A signed download URL is used only in memory. Strip auth
                    # even on same-host redirects; never follow credentialed URLs.
                    headers = {"Accept": "application/octet-stream", "User-Agent": "CMMUU-Gitee-Sync/1"}
                    url = location
            size, hasher = 0, hashlib.sha256()
            with response, part.open("xb") as stream:
                while True:
                    block = response.read(1024 * 1024)
                    if not block:
                        break
                    size += len(block)
                    if size > expected_size:
                        raise SyncError("Attachment is larger than source metadata")
                    hasher.update(block)
                    stream.write(block)
            actual = hasher.hexdigest()
            if size != expected_size or (expected_sha is not None and actual != expected_sha):
                raise SyncError("Attachment size or SHA-256 validation failed")
            part.rename(destination)
            return actual
        except (URLError, TimeoutError, OSError, ValueError, http.client.HTTPException):
            raise SyncError(f"{self.service} attachment transfer failed; safe to rerun") from None
        finally:
            if part.exists():
                part.unlink()

    def upload(self, path, file):
        if self.service != "gitee" or "?" in path:
            raise SyncError("Uploads are restricted to the Gitee API")
        file = Path(file)
        name = safe_name(file.name)
        boundary = "gitee-sync-" + uuid.uuid4().hex
        start = (f"--{boundary}\r\nContent-Disposition: form-data; name=\"access_token\"\r\n\r\n".encode()
                 + self.token.encode() + f"\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n".encode())
        end = f"\r\n--{boundary}--\r\n".encode()
        conn = http.client.HTTPSConnection("gitee.com", timeout=180)
        try:
            conn.putrequest("POST", "/api/v5" + path)
            conn.putheader("Content-Type", "multipart/form-data; boundary=" + boundary)
            conn.putheader("Content-Length", str(len(start) + file.stat().st_size + len(end)))
            conn.putheader("User-Agent", "CMMUU-Gitee-Sync/1")
            conn.endheaders()
            conn.send(start)
            with file.open("rb") as stream:
                for block in iter(lambda: stream.read(1024 * 1024), b""):
                    conn.send(block)
            conn.send(end)
            response = conn.getresponse()
            raw = response.read(MAX_JSON + 1)
            if response.status != 201 or len(raw) > MAX_JSON:
                raise SyncError(f"Gitee upload returned HTTP {response.status}; do not blindly retry POST")
            return json.loads(raw)
        except (OSError, ValueError, http.client.HTTPException):
            raise SyncError("Gitee upload outcome is uncertain; rerun to inspect existing attachments") from None
        finally:
            conn.close()


def verify_manifest(files):
    manifests = [item for item in files if item["name"] == "SHA256SUMS.txt"]
    if not manifests:
        return
    path = Path(manifests[0]["path"])
    if path.stat().st_size > 65536:
        raise SyncError("SHA256SUMS.txt exceeds the expected size")
    rows = {}
    for line in path.read_text(encoding="utf-8-sig").splitlines():
        if not line:
            continue
        match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *](.+)", line)
        if not match or match[2] in rows:
            raise SyncError("Malformed or ambiguous SHA256SUMS.txt")
        rows[safe_name(match[2])] = match[1].lower()
    expected = {item["name"]: item["sha256"] for item in files if item not in manifests}
    if rows != expected:
        raise SyncError("SHA256SUMS.txt does not exactly match the release attachments")


def git_credential():
    """Invoked ONLY by Git; stdout is the credential pipe, never a normal log."""
    fields = dict(line.rstrip("\n").split("=", 1) for line in sys.stdin if "=" in line)
    repo = os.environ.get("SYNC_REPO", "")
    host, path = fields.get("host"), fields.get("path", "").removesuffix(".git")
    owner = GH_OWNER if host == "github.com" else GE_OWNER
    if repo not in REPOS or fields.get("protocol") != "https" or host not in {"github.com", "gitee.com"} or path.casefold() != f"{owner}/{repo}".casefold():
        return
    token = os.environ.get("GITHUB_TOKEN" if host == "github.com" else "GITEE_TOKEN", "")
    if token and sys.argv[-1] == "get":
        username = "x-access-token" if host == "github.com" else GE_OWNER
        sys.stdout.write(f"username={username}\npassword={token}\n\n")


def git_run(repo, *args):
    env = os.environ.copy()
    env.update({"SYNC_REPO": repo, "GIT_TERMINAL_PROMPT": "0", "GCM_INTERACTIVE": "Never",
                "GIT_TRACE": "0", "GIT_TRACE_CURL": "0", "GIT_CURL_VERBOSE": "0",
                "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_SYSTEM": os.devnull,
                "GIT_ALLOW_PROTOCOL": "https"})
    helper = "!" + shlex.quote(Path(sys.executable).as_posix()) + " " + shlex.quote(Path(__file__).resolve().as_posix()) + " _git_credential"
    command = ["git", "-c", "credential.helper=", "-c", "credential.helper=" + helper,
               "-c", "credential.useHttpPath=true", "-c", "core.askPass=", *args]
    try:
        result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=300)
    except (OSError, subprocess.TimeoutExpired):
        raise SyncError("Git operation failed or timed out; no credential output is logged") from None
    if result.returncode:
        raise SyncError("Git operation failed (network, authorization, conflicting refs or unsupported atomic push); remote refs were not forced")
    return result.stdout.strip()


class Sync:
    def __init__(self, repo, github, gitee, work):
        self.repo, self.gh, self.ge, self.work = repo, github, gitee, Path(work)
        if repo not in REPOS:
            raise SyncError("Unsupported repository")
        self.source_path = f"/repos/{GH_OWNER}/{repo}"
        self.target_path = f"/repos/{GE_OWNER}/{repo}"
        self.source = None

    def guard(self):
        self.source = self.gh.request(self.source_path)
        target = self.ge.request(self.target_path)
        validate_pair(self.repo, self.source, target)
        # Public repository metadata can succeed without valid credentials.
        # /user must authenticate the intended owner before any external write.
        identity = self.ge.request("/user")
        if str(identity.get("login", "")).casefold() != GE_OWNER.casefold():
            raise SyncError("Gitee credential identity must match the permitted target owner")
        return target

    def sync_refs(self):
        self.guard()
        self.work.mkdir(parents=True, exist_ok=True)
        bare = self.work / (self.repo + ".git")
        source_url = f"https://github.com/{GH_OWNER}/{self.repo}.git"
        if bare.exists():
            if (bare.is_symlink() or git_run(self.repo, "--git-dir", str(bare), "rev-parse", "--is-bare-repository") != "true"
                    or git_run(self.repo, "--git-dir", str(bare), "remote", "get-url", "origin") != source_url):
                raise SyncError("Existing working mirror does not match the source repository")
            git_run(self.repo, "--git-dir", str(bare), "fetch", "--prune", "origin")
        else:
            git_run(self.repo, "clone", "--mirror", source_url, str(bare))
        self.guard()  # Recheck privacy immediately before the first external write.
        destination = f"https://gitee.com/{GE_OWNER}/{self.repo}.git"
        # Explicit namespaces: no deletion, force push, pull refs or remote configs.
        git_run(self.repo, "--git-dir", str(bare), "push", "--atomic", destination,
                "refs/heads/*:refs/heads/*", "refs/tags/*:refs/tags/*")
        expected = dict(line.split(" ", 1) for line in git_run(self.repo, "--git-dir", str(bare), "for-each-ref",
                        "--format=%(refname) %(objectname)", "refs/heads", "refs/tags").splitlines())
        actual = {ref: sha for sha, ref in (line.split() for line in git_run(self.repo, "ls-remote", "--refs", destination).splitlines())}
        if any(actual.get(ref) != sha for ref, sha in expected.items()):
            raise SyncError("Gitee branches or tags did not match the source after push")
        return bare

    def ensure_attachment(self, release_id, item):
        endpoint = f"{self.target_path}/releases/{release_id}/attach_files"
        matches = [asset for asset in self.ge.pages(endpoint) if asset.get("name") == item["name"]]
        if len(matches) > 1:
            raise SyncError("Duplicate destination attachment names; no files were replaced")
        if not matches:
            self.guard()
            if Path(item["path"]).stat().st_size != item["size"] or sha256(item["path"]) != item["sha256"]:
                raise SyncError("Local source attachment changed before upload")
            self.ge.upload(endpoint, item["path"])
            matches = [asset for asset in self.ge.pages(endpoint) if asset.get("name") == item["name"]]
            if len(matches) != 1:
                raise SyncError("Upload completed without one unambiguous destination attachment")
        asset = matches[0]
        if type(asset.get("id")) is not int or (type(asset.get("size")) is int and asset["size"] != item["size"]):
            raise SyncError("Destination attachment metadata differs; it was not replaced")
        # Gitee AttachFile has no documented digest. Compare downloaded bytes,
        # including existing same-name attachments, instead of trusting the name.
        check = self.work / "verify" / str(release_id) / (str(asset["id"]) + ".verify-" + uuid.uuid4().hex)
        try:
            self.ge.download(f"{endpoint}/{asset['id']}/download", check, item["size"], item["sha256"])
        finally:
            if check.exists():
                check.unlink()

    def sync_release(self, release, bare):
        if release.get("draft"):
            return  # Gitee release API has no documented equivalent of GitHub drafts.
        tag = release.get("tag_name", "")
        if not isinstance(tag, str) or not tag or "\x00" in tag:
            raise SyncError("Invalid release tag")
        commit = git_run(self.repo, "--git-dir", str(bare), "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}")
        if not re.fullmatch(r"[0-9a-f]{40,64}", commit):
            raise SyncError("Release tag does not resolve to a source commit")
        assets = self.gh.pages(f"{self.source_path}/releases/{release['id']}/assets")
        files, names = [], set()
        directory = self.work / "assets" / self.repo / str(release["id"])
        for asset in assets:
            name = safe_name(asset.get("name"))
            if name.casefold() in names or asset.get("state") != "uploaded" or type(asset.get("id")) is not int:
                raise SyncError("Incomplete or ambiguous GitHub attachment")
            names.add(name.casefold())
            api_path = f"{self.source_path}/releases/assets/{asset['id']}"
            if asset.get("url") != GH_API + api_path:
                raise SyncError("GitHub attachment belongs to a different repository")
            file = directory / name
            actual = self.gh.download(api_path, file, asset["size"], source_digest(asset))
            files.append({"name": name, "path": file, "size": asset["size"], "sha256": actual})
        verify_manifest(files)
        matches = [row for row in self.ge.pages(self.target_path + "/releases") if row.get("tag_name") == tag]
        if len(matches) > 1:
            raise SyncError("Duplicate destination releases for one tag")
        metadata = {"tag_name": tag, "name": release.get("name") or tag, "body": release.get("body") or "",
                    "prerelease": bool(release.get("prerelease")), "target_commitish": commit}
        self.guard()
        if not matches:
            # Gitee has no draft releases. Keep a new release in prerelease state
            # until every attachment has been uploaded and downloaded to verify.
            # A failed run can resume the same release without advertising an
            # incomplete stable update to the application.
            target = self.ge.request(self.target_path + "/releases", "POST", {**metadata, "prerelease": "true"})
        else:
            target = matches[0]
        if type(target.get("id")) is not int or target["id"] <= 0 or target.get("tag_name") != tag:
            raise SyncError("Gitee release response does not match the source tag")
        for item in files:
            self.ensure_attachment(target["id"], item)
        if any(target.get(key) != metadata[key] for key in ("name", "body", "prerelease")):
            self.guard()
            release_id = target["id"]
            target = self.ge.request(f"{self.target_path}/releases/{release_id}", "PATCH",
                                     {key: str(value).lower() if type(value) is bool else value for key, value in metadata.items() if key != "target_commitish"})
            if target.get("id") != release_id:
                raise SyncError("Gitee release update returned a different release")
        if any(target.get(key) != metadata[key] for key in ("tag_name", "name", "body", "prerelease")):
            raise SyncError("Gitee release metadata does not match the source after synchronization")
        print(f"Synchronized {self.repo}: {tag}, {len(files)} attachment(s)", flush=True)

    def run(self, scope, apply):
        self.guard()
        releases = self.gh.pages(self.source_path + "/releases") if scope == "all" else []
        if not apply:
            print(f"Preflight passed for {self.repo}; {len(releases)} release(s) found. No external changes without --apply.")
            return
        bare = self.sync_refs()
        for release in reversed(releases):
            self.sync_release(release, bare)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", choices=sorted(REPOS), required=True)
    parser.add_argument("--scope", choices=("refs", "all"), default="all")
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--apply", action="store_true", help="Explicitly authorize writes to the checked Gitee repository")
    args = parser.parse_args()
    gh_token, ge_token = os.environ.get("GITHUB_TOKEN"), os.environ.get("GITEE_TOKEN")
    if not gh_token:
        raise SyncError("Missing GITHUB_TOKEN environment variable")
    if not ge_token:
        raise SyncError("Missing GITEE_TOKEN. Add it once in the GitHub repository Settings > Secrets and variables > Actions, then rerun this workflow")
    extra_hosts = {value.strip().lower() for value in os.environ.get("GITEE_ASSET_HOSTS", "").split(",") if value.strip()}
    if any(not re.fullmatch(r"[a-z0-9]+(?:[.-][a-z0-9]+)*", host) for host in extra_hosts):
        raise SyncError("GITEE_ASSET_HOSTS accepts exact hostnames only, without wildcards or URLs")
    Sync(args.repo, Api("github", gh_token), Api("gitee", ge_token, extra_hosts), args.work_dir).run(args.scope, args.apply)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "_git_credential":
        git_credential()
    else:
        try:
            main()
        except SyncError as error:
            print(f"Sync stopped: {error}", file=sys.stderr)
            raise SystemExit(1)
