#!/usr/bin/env python3
"""Validate giclean release inputs and published assets without publishing anything."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib


REPO = "kosnu/giclean"
TARGETS = (
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def output(*args):
    return subprocess.run(args, check=True, text=True, capture_output=True).stdout


def api(path):
    return json.loads(output("gh", "api", f"repos/{REPO}/{path}"))


def check_source(version, commit):
    manifest = tomllib.loads(output("git", "show", f"{commit}:Cargo.toml"))
    lock = tomllib.loads(output("git", "show", f"{commit}:Cargo.lock"))
    require(manifest["package"]["name"] == "giclean", "unexpected package")
    require(manifest["package"]["version"] == version, "Cargo.toml version mismatch")
    packages = [p for p in lock["package"] if p["name"] == "giclean"]
    require(len(packages) == 1 and packages[0]["version"] == version,
            "Cargo.lock version mismatch")


def check_published(version, commit, run_id):
    tag = f"v{version}"
    ref = f"refs/tags/{tag}"
    lines = output("git", "ls-remote", "--tags", "origin", ref, ref + "^{}").splitlines()
    refs = dict(line.split()[::-1] for line in lines)
    require(ref in refs and refs.get(ref + "^{}") == commit,
            "remote annotated tag does not point to the expected commit")

    run = api(f"actions/runs/{run_id}")
    require(run["id"] == run_id and run["head_sha"] == commit,
            "Release run ID or commit mismatch")
    require(run["path"] == ".github/workflows/release.yml"
            and run["event"] == "push" and run["head_branch"] == tag,
            "run is not the tag-triggered Release workflow")
    require(run["status"] == "completed" and run["conclusion"] == "success",
            "Release has not completed successfully")

    release = api(f"releases/tags/{tag}")
    prerelease = "-" in version.split("+", 1)[0]
    require(release["tag_name"] == tag and release["draft"] is False
            and bool(release["published_at"]), "release is not published")
    require(release["prerelease"] is prerelease, "prerelease flag mismatch")
    require(bool(release["body"].strip()), "release notes are empty")
    archives = {f"giclean-{tag}-{target}.tar.gz" for target in TARGETS}
    expected = archives | {"SHA256SUMS"}
    assets = release["assets"]
    require(len(assets) == len(expected) and {a["name"] for a in assets} == expected,
            "release asset names or count mismatch")
    require(all(a["state"] == "uploaded" and a["size"] > 0 for a in assets),
            "release assets are incomplete")

    with tempfile.TemporaryDirectory(prefix="giclean-release-") as directory:
        output("gh", "release", "download", tag, "--repo", REPO, "--dir", directory)
        root = Path(directory)
        require({p.name for p in root.iterdir()} == expected,
                "downloaded asset names mismatch")
        sums = {}
        for line in (root / "SHA256SUMS").read_text().splitlines():
            match = re.fullmatch(r"([0-9a-f]{64})  (\S+)", line)
            require(match is not None, "invalid SHA256SUMS line")
            digest, name = match.groups()
            require(name in archives and name not in sums, "unexpected or duplicate checksum")
            sums[name] = digest
        require(set(sums) == archives, "missing checksum")
        for name in sorted(archives):
            require(hashlib.sha256((root / name).read_bytes()).hexdigest() == sums[name],
                    f"checksum mismatch: {name}")
    return release["html_url"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("source", "published"))
    parser.add_argument("version", help="package version without v")
    parser.add_argument("commit", help="full expected commit SHA")
    parser.add_argument("--run-id", type=int)
    args = parser.parse_args()
    if not re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", args.version):
        parser.error("version must be X.Y.Z with optional prerelease/build suffix")
    if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", args.commit):
        parser.error("commit must be a full SHA")
    if args.mode == "published" and (args.run_id is None or args.run_id <= 0):
        parser.error("published requires a positive --run-id")
    try:
        check_source(args.version, args.commit)
        if args.mode == "published":
            print(check_published(args.version, args.commit, args.run_id))
        else:
            print(f"OK: v{args.version} at {args.commit}")
    except (ValueError, KeyError, TypeError, OSError, subprocess.CalledProcessError) as error:
        detail = error.stderr if isinstance(error, subprocess.CalledProcessError) else str(error)
        parser.exit(1, f"Release check failed: {detail}\n")


if __name__ == "__main__":
    main()
