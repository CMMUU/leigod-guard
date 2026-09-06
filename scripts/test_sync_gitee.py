"""Offline boundaries only: no tokens, accounts, Git transport, or remote writes."""
import hashlib
import http.client
from io import BytesIO
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

import sync_gitee as sync


HERE = Path(__file__).resolve().parent


def metadata(private=False, owner="cmmuu", repo="mihomo-codex"):
    return {"full_name": f"{owner}/{repo}", "owner": {"login": owner}, "private": private,
            "path": repo, "html_url": f"https://gitee.com/{owner}/{repo}"}


class Response(BytesIO):
    pass


class Opener:
    def __init__(self, results):
        self.results, self.requests = list(results), []

    def open(self, request, timeout):
        self.requests.append(request)
        result = self.results.pop(0)
        if isinstance(result, Exception):
            raise result
        return result


class GiteeFixture:
    def __init__(self, content=None, copies=1):
        self.content, self.copies = content, copies
        self.uploads = 0

    def pages(self, path):
        if self.content is None:
            return []
        return [{"id": index + 1, "name": "package.zip", "size": len(self.content)} for index in range(self.copies)]

    def upload(self, path, file):
        self.uploads += 1
        self.content = Path(file).read_bytes()

    def download(self, path, destination, expected_size, expected_sha):
        if len(self.content) != expected_size or hashlib.sha256(self.content).hexdigest() != expected_sha:
            raise sync.SyncError("Attachment size or SHA-256 validation failed")
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(self.content)
        return expected_sha


class SyncTests(unittest.TestCase):
    def fixture(self):
        temp = tempfile.TemporaryDirectory(prefix="offline-sync-", dir=HERE)
        self.addCleanup(temp.cleanup)
        return Path(temp.name)

    def test_private_to_public_is_blocked_before_git_or_release_writes(self):
        class GH:
            def request(self, path):
                return metadata(True, "CMMUU")
        class GE:
            def request(self, path):
                if path == "/user":
                    return {"login": "cmmuu"}
                return metadata(False)
        job = sync.Sync("mihomo-codex", GH(), GE(), self.fixture())
        with patch.object(sync, "git_run") as git:
            with self.assertRaisesRegex(sync.SyncError, "Private GitHub"):
                job.sync_refs()
            git.assert_not_called()

    def test_wrong_owner_name_and_unknown_visibility_fail_closed(self):
        source = metadata(True, "CMMUU")
        for target in (metadata(True, "other"), metadata(True, repo="other"), metadata("false")):
            with self.subTest(target=target):
                with self.assertRaises(sync.SyncError):
                    sync.validate_pair("mihomo-codex", source, target)
        sync.validate_pair("mihomo-codex", source, metadata(True))

    def test_scope_diagnostic_names_fields_without_echoing_response_values(self):
        source = metadata(False, "CMMUU")
        target = metadata(False)
        target.pop("path")
        with self.assertRaisesRegex(sync.SyncError, "path=missing, html_url=matches"):
            sync.validate_pair("mihomo-codex", source, target)
        target["path"] = "private-test-value"
        target["html_url"] = "https://gitee.com/file?secret=private-test-value"
        with self.assertRaisesRegex(sync.SyncError, "path=differs, html_url=differs") as error:
            sync.validate_pair("mihomo-codex", source, target)
        self.assertNotIn("private-test-value", str(error.exception))

    def test_gitee_clone_form_html_url_identifies_only_the_checked_repository(self):
        source = metadata(False, "CMMUU", "leigod-guard")
        target = metadata(False, repo="leigod-guard")
        for address in ("https://gitee.com/cmmuu/leigod-guard", "https://gitee.com/cmmuu/leigod-guard.git",
                        "https://gitee.com/CMMUU/leigod-guard.git/"):
            target["html_url"] = address
            sync.validate_pair("leigod-guard", source, target)
        for address in ("http://gitee.com/cmmuu/leigod-guard.git", "https://gitee.com/other/leigod-guard.git",
                        "https://gitee.com/cmmuu/other.git", "https://gitee.com.attacker.example/cmmuu/leigod-guard.git",
                        "https://user@gitee.com/cmmuu/leigod-guard.git", "https://gitee.com:444/cmmuu/leigod-guard.git",
                        "https://gitee.com/cmmuu/leigod-guard.git?secret=private-test-value",
                        "https://gitee.com/cmmuu/leigod-guard.git#main", "https://gitee.com/cmmuu/leigod-guard.git/tree/main"):
            target["html_url"] = address
            with self.subTest(address=address):
                with self.assertRaises(sync.SyncError):
                    sync.validate_pair("leigod-guard", source, target)

    def test_public_repository_does_not_bypass_authenticated_owner_preflight(self):
        class GH:
            def request(self, path):
                return metadata(False, "CMMUU", "leigod-guard")
        class GE:
            identity = {"login": "other-owner"}
            def request(self, path):
                if path == "/user":
                    if isinstance(self.identity, Exception):
                        raise self.identity
                    return self.identity
                return metadata(False, repo="leigod-guard")
        ge = GE()
        job = sync.Sync("leigod-guard", GH(), ge, self.fixture())
        with patch.object(sync, "git_run") as git:
            for identity in ({}, {"login": "other-owner"}, sync.SyncError("Bearer rejected")):
                ge.identity = identity
                with self.assertRaises(sync.SyncError):
                    job.sync_refs()
            git.assert_not_called()
        ge.identity = {"login": "CMMUU"}
        self.assertEqual(job.guard()["path"], "leigod-guard")

    def test_redirect_never_forwards_auth_and_unknown_host_is_rejected(self):
        data = b"verified package"
        api = sync.Api("github", "offline-secret")
        api.opener = Opener([
            HTTPError("https://api.github.com/asset", 302, "redirect", {"Location": "https://release-assets.githubusercontent.com/file?signature=example"}, None),
            Response(data),
        ])
        path = self.fixture() / "file.zip"
        api.download("/repos/CMMUU/leigod-guard/releases/assets/1", path, len(data), hashlib.sha256(data).hexdigest())
        self.assertEqual(api.opener.requests[0].get_header("Authorization"), "Bearer offline-secret")
        self.assertIsNone(api.opener.requests[1].get_header("Authorization"))
        self.assertEqual(path.read_bytes(), data)
        api.opener = Opener([HTTPError("https://api.github.com/asset", 302, "redirect", {"Location": "https://attacker.example/file"}, None)])
        with self.assertRaisesRegex(sync.SyncError, "Untrusted"):
            api.download("/asset", self.fixture() / "file.zip", 1, "a" * 64)
        self.assertEqual(len(api.opener.requests), 1)
        for url in ("http://gitee.com/file", "https://gitee.com.attacker.example/file", "https://user@gitee.com/file", "https://gitee.com:444/file"):
            with self.assertRaises(sync.SyncError):
                sync.checked_url(url, {"gitee.com"})

    def test_corrupt_or_truncated_download_never_becomes_final_file(self):
        for payload in (b"bad", b"good-extra", b"goo"):
            api = sync.Api("github", "offline-secret")
            api.opener = Opener([Response(payload)])
            directory = self.fixture()
            with self.assertRaises(sync.SyncError):
                api.download("/asset", directory / "file.zip", 4, hashlib.sha256(b"good").hexdigest())
            self.assertEqual(list(directory.iterdir()), [])

    def test_protocol_errors_do_not_expose_signed_urls_or_credentials(self):
        for binary in (False, True):
            api = sync.Api("github", "offline-secret")
            api.opener = Opener([http.client.InvalidURL("signed-url?secret=offline-secret")])
            with self.assertRaises(sync.SyncError) as error:
                if binary:
                    api.download("/asset", self.fixture() / "file.zip", 1, "a" * 64)
                else:
                    api.request("/repos/CMMUU/leigod-guard")
            self.assertNotIn("offline-secret", str(error.exception))
            self.assertNotIn("signed-url", str(error.exception))

    def attachment_job(self, existing=None, copies=1):
        root = self.fixture()
        source = root / "package.zip"
        source.write_bytes(b"good")
        ge = GiteeFixture(existing, copies)
        job = sync.Sync("mihomo-codex", None, ge, root)
        job.guard = lambda: None
        item = {"path": source, "name": source.name, "size": 4, "sha256": hashlib.sha256(b"good").hexdigest()}
        return job, ge, item

    def test_identical_existing_attachment_is_verified_and_not_uploaded_again(self):
        job, ge, item = self.attachment_job(b"good")
        job.ensure_attachment(1, item)
        self.assertEqual(ge.uploads, 0)

    def test_conflicting_or_duplicate_attachment_is_never_overwritten(self):
        for content, copies in ((b"evil", 1), (b"good", 2)):
            job, ge, item = self.attachment_job(content, copies)
            with self.assertRaises(sync.SyncError):
                job.ensure_attachment(1, item)
            self.assertEqual(ge.uploads, 0)
            self.assertEqual(ge.content, content)

    def test_missing_attachment_uploads_once_then_is_download_verified_and_reusable(self):
        job, ge, item = self.attachment_job()
        job.ensure_attachment(1, item)
        job.ensure_attachment(1, item)
        self.assertEqual(ge.uploads, 1)
        self.assertEqual(ge.content, b"good")

    def test_privacy_recheck_blocks_upload_when_visibility_changes(self):
        job, ge, item = self.attachment_job()
        def changed():
            sync.validate_pair("mihomo-codex", metadata(True, "CMMUU"), metadata(False))
        job.guard = changed
        with self.assertRaisesRegex(sync.SyncError, "Private GitHub"):
            job.ensure_attachment(1, item)
        self.assertEqual(ge.uploads, 0)

    def test_checksum_manifest_cannot_substitute_another_name_or_hash(self):
        directory = self.fixture()
        manifest = directory / "SHA256SUMS.txt"
        expected = hashlib.sha256(b"good").hexdigest()
        rows = [{"name": "SHA256SUMS.txt", "path": manifest}, {"name": "package.zip", "sha256": expected}]
        manifest.write_text(expected + "  package.zip\n", encoding="utf-8")
        sync.verify_manifest(rows)
        for value in (expected + "  other.zip\n", "0" * 64 + "  package.zip\n", (expected + "  package.zip\n") * 2):
            manifest.write_text(value, encoding="utf-8")
            with self.assertRaises(sync.SyncError):
                sync.verify_manifest(rows)

    def test_github_write_is_rejected_without_contacting_network(self):
        api = sync.Api("github", "offline-secret")
        api.opener = Opener([])
        with self.assertRaisesRegex(sync.SyncError, "GitHub writes"):
            api.request("/repos/CMMUU/leigod-guard/releases", "DELETE")
        self.assertEqual(api.opener.requests, [])

    def test_git_failure_diagnostic_does_not_echo_credentials_or_raw_output(self):
        for message, expected in (("fatal: --mirror can't be combined with refspecs", "mirror/refspec conflict"),
                                  ("fatal: Authentication failed", "authorization rejected"),
                                  ("error: RPC failed", "network transport failure")):
            result = sync.subprocess.CompletedProcess([], 1, stdout="offline-secret",
                stderr=message + " https://user:offline-secret@gitee.com/private-value")
            with patch.object(sync.subprocess, "run", return_value=result):
                with self.assertRaisesRegex(sync.SyncError, "Git push failed") as error:
                    sync.git_run("leigod-guard", "push", "https://gitee.com/cmmuu/leigod-guard.git")
            self.assertIn(expected, str(error.exception))
            self.assertNotIn("offline-secret", str(error.exception))
            self.assertNotIn("private-value", str(error.exception))

    def test_only_read_only_network_failure_is_retried_once(self):
        failed = sync.subprocess.CompletedProcess([], 1, stdout="", stderr="error: RPC failed")
        success = sync.subprocess.CompletedProcess([], 0, stdout="verified refs\n", stderr="")
        with patch.object(sync.subprocess, "run", side_effect=[failed, success]) as run:
            result = sync.git_run("leigod-guard", "ls-remote", "--refs", "https://gitee.com/cmmuu/leigod-guard.git")
            self.assertEqual(result, "verified refs")
            self.assertEqual(run.call_count, 2)
            self.assertEqual(run.call_args_list[0], run.call_args_list[1])
            self.assertIn("http.https://gitee.com/.version=HTTP/1.1", run.call_args.args[0])
        for operation, failure, expected_calls in (("push", failed, 1), ("ls-remote", failed, 2),
                ("ls-remote", sync.subprocess.CompletedProcess([], 1, stdout="", stderr="Authentication failed"), 1)):
            with patch.object(sync.subprocess, "run", return_value=failure) as run:
                with self.assertRaises(sync.SyncError):
                    sync.git_run("leigod-guard", operation, "https://gitee.com/cmmuu/leigod-guard.git")
                self.assertEqual(run.call_count, expected_calls)

    def test_ref_sync_copies_all_heads_and_tags_without_force_or_remote_deletion(self):
        job = sync.Sync("mihomo-codex", None, None, self.fixture())
        job.guard = lambda: None
        calls = []
        refs = {"refs/heads/main": "a" * 40, "refs/heads/topic": "b" * 40, "refs/tags/v0.4.0": "c" * 40}
        def git(repo, *args):
            calls.append(args)
            if "for-each-ref" in args:
                return "\n".join(f"{ref} {sha}" for ref, sha in refs.items())
            if "ls-remote" in args:
                if not any("push" in call for call in calls):
                    return ""
                return "\n".join(f"{sha}\t{ref}" for ref, sha in refs.items())
            return ""
        with patch.object(sync, "git_run", side_effect=git):
            job.sync_refs()
        pushes = [args for args in calls if "push" in args]
        self.assertEqual(len(pushes), 1)
        self.assertEqual(pushes[0][-2:], ("refs/heads/*:refs/heads/*", "refs/tags/*:refs/tags/*"))
        self.assertIn("--atomic", pushes[0])
        self.assertFalse(any(option in pushes[0] for option in ("--force", "--mirror", "--prune", "--delete")))

    def test_matching_refs_skip_push_and_uncertain_push_requires_full_verification(self):
        refs = {"refs/heads/main": "a" * 40, "refs/tags/v0.11.0": "b" * 40}
        for initial_match, final_state in ((True, "match"), (False, "match"),
                                           (False, "missing-tag"), (False, "wrong-branch"),
                                           (False, "read-failed")):
            with self.subTest(initial_match=initial_match, final_state=final_state):
                job = sync.Sync("leigod-guard", None, None, self.fixture())
                job.guard = lambda: None
                calls = []
                def git(repo, *args):
                    calls.append(args)
                    if "for-each-ref" in args:
                        return "\n".join(f"{ref} {sha}" for ref, sha in refs.items())
                    if "push" in args:
                        raise sync.SyncError("Uncertain transport result")
                    if "ls-remote" in args:
                        if not initial_match and not any("push" in call for call in calls):
                            return ""
                        if final_state == "read-failed":
                            raise sync.SyncError("Verification read failed")
                        actual = dict(refs)
                        actual["refs/heads/gitee-only"] = "d" * 40
                        if final_state == "missing-tag":
                            actual.pop("refs/tags/v0.11.0")
                        elif final_state == "wrong-branch":
                            actual["refs/heads/main"] = "c" * 40
                        return "\n".join(f"{sha}\t{ref}" for ref, sha in actual.items())
                    return ""
                with patch.object(sync, "git_run", side_effect=git):
                    if final_state == "match":
                        job.sync_refs()
                    else:
                        with self.assertRaises(sync.SyncError):
                            job.sync_refs()
                self.assertEqual(sum("push" in call for call in calls), 0 if initial_match else 1)

    def test_release_title_body_and_attachment_are_copied_once_then_reused(self):
        data = b"good"
        digest = hashlib.sha256(data).hexdigest()
        class GH:
            def request(self, path):
                return metadata(True, "CMMUU")
            def pages(self, path):
                return [{"id": 7, "name": "package.zip", "state": "uploaded", "size": 4,
                         "digest": "sha256:" + digest,
                         "url": "https://api.github.com/repos/CMMUU/mihomo-codex/releases/assets/7"}]
            def download(self, path, destination, expected_size, expected_sha):
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(data)
                return digest
        class GE(GiteeFixture):
            def __init__(self):
                super().__init__()
                self.release, self.creates, self.updates = None, 0, 0
            def request(self, path, method="GET", data=None):
                if method == "GET":
                    if path == "/user":
                        return {"login": "cmmuu"}
                    return metadata(True)
                if method == "POST":
                    self.creates += 1
                    self.release = {**data, "id": 12, "prerelease": data["prerelease"] == "true"}
                elif method == "PATCH":
                    self.updates += 1
                    self.release.update(data)
                    self.release["prerelease"] = data["prerelease"] == "true"
                return self.release
            def upload(self, path, file):
                if not self.release["prerelease"]:
                    raise AssertionError("New release was stable before all attachments were verified")
                super().upload(path, file)
            def pages(self, path):
                if path.endswith("/releases"):
                    return [self.release] if self.release else []
                return super().pages(path)
        ge = GE()
        job = sync.Sync("mihomo-codex", GH(), ge, self.fixture())
        release = {"id": 1, "tag_name": "v0.4.0", "name": "准确标题", "body": "原正文\n第二行", "prerelease": False, "draft": False}
        with patch.object(sync, "git_run", return_value="a" * 40), patch("builtins.print"):
            job.sync_release(release, job.work / "fixture.git")
            job.sync_release(release, job.work / "fixture.git")
        self.assertEqual((ge.creates, ge.updates, ge.uploads), (1, 1, 1))
        self.assertEqual(ge.release["name"], release["name"])
        self.assertEqual(ge.release["body"], release["body"])
        self.assertEqual(ge.release["target_commitish"], "a" * 40)
        self.assertFalse(ge.release["prerelease"])

        # A failed attachment verification leaves the new release as a preview;
        # a rerun verifies the existing upload and only then promotes it.
        ge = GE()
        job = sync.Sync("mihomo-codex", GH(), ge, self.fixture())
        with patch.object(sync, "git_run", return_value="a" * 40), patch("builtins.print"):
            with patch.object(ge, "download", side_effect=sync.SyncError("transfer failed")):
                with self.assertRaisesRegex(sync.SyncError, "transfer failed"):
                    job.sync_release(release, job.work / "fixture.git")
            self.assertTrue(ge.release["prerelease"])
            self.assertEqual(ge.updates, 0)
            job.sync_release(release, job.work / "fixture.git")
        self.assertFalse(ge.release["prerelease"])
        self.assertEqual((ge.creates, ge.updates, ge.uploads), (1, 1, 1))


if __name__ == "__main__":
    unittest.main()
