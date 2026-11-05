import hashlib
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

import check_release as release


class ReleaseChecks(unittest.TestCase):
    def setUp(self):
        self.version = "0.3.1"
        self.commit = "a" * 40
        self.run = dict(id=123, head_sha=self.commit, path=".github/workflows/release.yml",
                        event="push", head_branch="v0.3.1", status="completed", conclusion="success")
        self.metadata = dict(tag_name="v0.3.1", draft=False, published_at="2026-11-05T00:00:00Z",
                             prerelease=False, body="Changes", html_url="https://example.test/release")
        self.corrupt = False
        self.bad_checksum = False
        self.remote_commit = self.commit
        self.lock_version = self.version
        self.configure_assets()

    def configure_assets(self):
        self.files = {f"giclean-v{self.version}-{target}.tar.gz": target.encode()
                      for target in release.TARGETS}
        self.metadata["assets"] = [dict(name=name, state="uploaded", size=len(data))
                                   for name, data in self.files.items()]
        self.metadata["assets"].append(dict(name="SHA256SUMS", state="uploaded", size=444))

    def output(self, *args):
        if args[:2] == ("git", "show"):
            if args[2].endswith(":Cargo.toml"):
                return f'[package]\nname = "giclean"\nversion = "{self.version}"\n'
            return f'[[package]]\nname = "giclean"\nversion = "{self.lock_version}"\n'
        if args[:2] == ("git", "ls-remote"):
            ref = f"refs/tags/v{self.version}"
            return f'{"b" * 40}\t{ref}\n{self.remote_commit}\t{ref}^{{}}\n'
        if args[:2] == ("gh", "api"):
            return json.dumps(self.run if "/actions/runs/" in args[2] else self.metadata)
        if args[:3] == ("gh", "release", "download"):
            root = Path(args[-1])
            for name, data in self.files.items():
                (root / name).write_bytes(data + (b"corrupt" if self.corrupt else b""))
            sums = "".join(f"{hashlib.sha256(data).hexdigest()}  {name}\n"
                           for name, data in self.files.items())
            (root / "SHA256SUMS").write_text("invalid\n" if self.bad_checksum else sums)
            return ""
        raise AssertionError(args)

    def check(self):
        with patch.object(release, "output", side_effect=self.output):
            release.check_source(self.version, self.commit)
            return release.check_published(self.version, self.commit, 123)

    def test_valid_release(self):
        self.assertEqual(self.check(), self.metadata["html_url"])

    def test_valid_prerelease(self):
        self.version = "0.4.0-rc.1"
        self.lock_version = self.version
        self.run["head_branch"] = "v" + self.version
        self.metadata.update(tag_name="v" + self.version, prerelease=True)
        self.configure_assets()
        self.assertEqual(self.check(), self.metadata["html_url"])

    def test_lock_mismatch(self):
        self.lock_version = "0.3.0"
        with self.assertRaisesRegex(ValueError, "Cargo.lock"):
            self.check()

    def test_wrong_tag_target(self):
        self.remote_commit = "c" * 40
        with self.assertRaisesRegex(ValueError, "annotated tag"):
            self.check()

    def test_wrong_or_unsuccessful_run(self):
        for key, value in (("head_sha", "c" * 40), ("head_branch", "main"),
                           ("path", ".github/workflows/ci.yml"), ("conclusion", "failure"),
                           ("status", "in_progress")):
            with self.subTest(key=key), patch.dict(self.run, {key: value}):
                with self.assertRaises(ValueError):
                    self.check()

    def test_unpublished_or_wrong_release(self):
        for key, value in (("draft", True), ("prerelease", True), ("published_at", None),
                           ("tag_name", "v0.3.0"), ("body", "")):
            with self.subTest(key=key), patch.dict(self.metadata, {key: value}):
                with self.assertRaises(ValueError):
                    self.check()

    def test_incomplete_assets(self):
        self.metadata["assets"].pop()
        with self.assertRaisesRegex(ValueError, "asset names"):
            self.check()

    def test_checksum_mismatch(self):
        self.corrupt = True
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.check()

    def test_malformed_checksum(self):
        self.bad_checksum = True
        with self.assertRaisesRegex(ValueError, "invalid SHA256SUMS"):
            self.check()

    def test_cli_rejects_abbreviated_commit_without_running_git(self):
        result = subprocess.run([sys.executable, str(Path(release.__file__)), "source", "0.3.1", "aaaa"],
                                text=True, capture_output=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn("full SHA", result.stderr)


if __name__ == "__main__":
    unittest.main()
