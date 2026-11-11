#!/usr/bin/env python3
"""Compare release binaries using real temporary Git repositories, without network IO."""

import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile
import time


def regressed(baseline, candidate):
    # Require both a relative and absolute increase to tolerate runner noise.
    return candidate > max(baseline * 1.30, baseline + 0.030)


class Repository:
    def __init__(self, root):
        self.path = root / "repo"
        self.origin = root / "origin.git"
        self.path.mkdir()
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith("GIT_")}
        self.env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                        GIT_TERMINAL_PROMPT="0", LC_ALL="C",
                        GIT_AUTHOR_NAME="Performance check",
                        GIT_AUTHOR_EMAIL="performance@example.invalid",
                        GIT_COMMITTER_NAME="Performance check",
                        GIT_COMMITTER_EMAIL="performance@example.invalid")
        self.git("init", "--initial-branch=main")
        self.git("init", "--bare", "--initial-branch=main", str(self.origin))
        self.git("remote", "add", "origin", str(self.origin))
        self.git("config", "core.hooksPath", str(root / "no-hooks"))
        self.git("config", "commit.gpgSign", "false")
        blob = self.git("hash-object", "-w", "--stdin", data="common\n")
        self.common = {f"file-{i:04}": blob for i in range(1000)}
        self.base = self.commit(self.common, "base")
        integrated = self.git("hash-object", "-w", "--stdin", data="integrated\n")
        target_files = self.common | {f"integrated-{i:03}": integrated for i in range(10)}
        self.target = self.commit(target_files, "integrations", self.base)
        self.equal = {f"equal-{i:03}": self.commit(
            self.common | {f"integrated-{i:03}": integrated}, f"equal {i}", self.base)
            for i in range(10)}
        self.unmerged = {}
        for i in range(100):
            blob = self.git("hash-object", "-w", "--stdin", data=f"unmerged {i}\n")
            self.unmerged[f"unmerged-{i:03}"] = self.commit(
                self.common | {f"unmerged-{i:03}": blob}, f"unmerged {i}", self.base)
        history = self.target
        for i in range(256):
            blob = self.git("hash-object", "-w", "--stdin", data=f"history {i}\n")
            history = self.commit(target_files | {"history": blob}, f"history {i}", history)
        later = self.git("hash-object", "-w", "--stdin", data="later changes\n")
        self.history = self.commit(
            target_files | {f"integrated-{i:03}": later for i in range(10)}, "later", history)

    def git(self, *args, data=None):
        result = subprocess.run(["git", *args], cwd=self.path, env=self.env,
                                input=None if data is None else data.encode(),
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        if result.returncode:
            raise RuntimeError(f"git {args[0]} failed: {result.stderr.decode(errors='replace')}")
        return result.stdout.decode().strip()

    def commit(self, files, message, parent=None):
        rows = "".join(f"100644 blob {blob}\t{name}\n" for name, blob in sorted(files.items()))
        tree = self.git("mktree", data=rows)
        return self.git("commit-tree", tree, *(["-p", parent] if parent else []), data=message)

    def cases(self):
        merged = {f"merged-{i:03}": self.base for i in range(20)}
        mixed = dict(list(self.equal.items())[:4] + list(self.unmerged.items())[:8])
        return [
            ("unmerged100", self.target, self.unmerged, []),
            ("equal10", self.target, self.equal, list(self.equal)),
            ("merged20", self.target, merged, list(merged)),
            ("mixed", self.target, mixed, list(self.equal)[:4]),
            ("historical10", self.history, self.equal, list(self.equal)),
        ]

    def heads(self):
        return dict(line.split(" ", 1) for line in self.git(
            "for-each-ref", "--format=%(refname) %(objectname)", "refs/heads").splitlines())

    def configure(self, target):
        # Setup and restoration are deliberately excluded from timing.
        self.git("update-ref", "refs/heads/main", target)
        self.git("reset", "--hard", "main")
        self.git("push", "--quiet", "origin", "main")

    def measure(self, binary, case):
        name, target, branches, deleted = case
        expected = {"refs/heads/main": target} | {
            f"refs/heads/{branch}": oid for branch, oid in branches.items()}
        commands = [f"delete {ref}" for ref in self.heads() if ref not in expected]
        commands += [f"update {ref} {oid}" for ref, oid in expected.items()]
        self.git("update-ref", "--stdin", data="\n".join(commands) + "\n")
        start = time.perf_counter()
        result = subprocess.run([str(binary), "br"], cwd=self.path, env=self.env,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15)
        seconds = time.perf_counter() - start
        if result.returncode or result.stderr:
            raise RuntimeError(f"{name}: {binary} failed: {result.stderr.decode(errors='replace')}")
        if sorted(result.stdout.decode().splitlines()) != sorted(f"{branch}: deleted" for branch in deleted):
            raise RuntimeError(f"{name}: unexpected deletion output from {binary}")
        for branch in deleted:
            del expected[f"refs/heads/{branch}"]
        if (self.heads() != expected or self.git("rev-parse", "HEAD") != target
                or self.git("status", "--porcelain")
                or (self.path / ".git/FETCH_HEAD").exists()):
            raise RuntimeError(f"{name}: unexpected repository changes from {binary}")
        return seconds


def compare(baseline, candidate, samples):
    results = []
    with tempfile.TemporaryDirectory(prefix="giclean-performance-") as directory:
        repo = Repository(Path(directory))
        for case in repo.cases():
            repo.configure(case[1])
            for binary in (baseline, candidate):
                repo.measure(binary, case)  # Warm up both versions once.
            timings = {"baseline": [], "candidate": []}
            for sample in range(samples):
                order = [("baseline", baseline), ("candidate", candidate)]
                for label, binary in order if sample % 2 == 0 else reversed(order):
                    timings[label].append(repo.measure(binary, case))
            before = statistics.median(timings["baseline"])
            after = statistics.median(timings["candidate"])
            results.append(dict(case=case[0], baseline_seconds=before, candidate_seconds=after,
                                change_percent=(after / before - 1) * 100,
                                regressed=regressed(before, after), samples=timings))
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--samples", type=int, default=7)
    parser.add_argument("--output", type=Path, default=Path("performance-results.json"))
    args = parser.parse_args()
    if args.samples < 3 or args.samples % 2 == 0:
        parser.error("--samples must be an odd number of at least 3")
    baseline, candidate = args.baseline.resolve(strict=True), args.candidate.resolve(strict=True)
    results = compare(baseline, candidate, args.samples)
    report = dict(baseline=str(baseline), candidate=str(candidate),
                  threshold="median increase >30% AND >30ms", results=results)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    table = ["### Wall-clock performance", "",
             "Failure: median increases by more than 30% AND 30ms. Local origin; network excluded.", "",
             "| Case | Baseline | Candidate | Change | Result |", "| --- | ---: | ---: | ---: | --- |"]
    for row in results:
        table.append(f"| {row['case']} | {row['baseline_seconds']:.3f}s | "
                     f"{row['candidate_seconds']:.3f}s | {row['change_percent']:+.1f}% | "
                     f"{'FAIL' if row['regressed'] else 'PASS'} |")
    summary = "\n".join(table) + "\n"
    print(summary)
    print(json.dumps(report, indent=2))
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as output:
            output.write(summary + "\n<details><summary>Samples (seconds)</summary>\n\n```json\n"
                         + json.dumps(report, indent=2) + "\n```\n</details>\n")
    return int(any(row["regressed"] for row in results))


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"Performance check failed: {error}", file=sys.stderr)
        sys.exit(1)
