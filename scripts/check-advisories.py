#!/usr/bin/env python3
"""Fail CI on any RustSec issue without an exact, unexpired, documented exception."""
import argparse
import datetime
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def check(report, exceptions, today):
    allowed = set()
    for entry in exceptions:
        if not entry.get("reason", "").strip() or not entry.get("advisory", "").startswith("RUSTSEC-"):
            raise ValueError("Each exception requires an advisory ID and a reachability/maintenance reason")
        if datetime.date.fromisoformat(entry["reviewBy"]) <= today:
            raise ValueError(f"Exception needs review: {entry['advisory']}")
        allowed.add((entry["advisory"], entry["package"], entry["version"]))
    # Require the normal audit schema; an unavailable database or broken audit
    # command cannot turn an empty/error JSON object into a successful check.
    issues = list(report["vulnerabilities"]["list"])
    for entries in report["warnings"].values():
        issues.extend(entries)
    failures = []
    for issue in issues:
        advisory = issue.get("advisory") or {}
        package = issue["package"]
        key = (advisory.get("id"), package["name"], package["version"])
        if key not in allowed:
            failures.append(f"{key[0] or 'yanked package'}: {key[1]} {key[2]}")
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, help="Use an existing cargo audit --json report")
    args = parser.parse_args()
    if args.report:
        report = json.loads(args.report.read_text())
    else:
        audit = subprocess.run(["cargo", "audit", "--json"], cwd=ROOT, capture_output=True, text=True)
        if audit.returncode not in (0, 1):
            raise ValueError(f"cargo audit failed: {audit.stderr}")
        report = json.loads(audit.stdout)
    exceptions = json.loads((ROOT / "docs/advisory-exceptions.json").read_text())
    failures = check(report, exceptions, datetime.date.today())
    for failure in failures:
        print(failure, file=sys.stderr)
    if failures:
        return 1
    print(f"Advisory check passed; {len(exceptions)} individually documented exceptions (see docs/advisory-exceptions.json)")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"Advisory check failed: {error}", file=sys.stderr)
        sys.exit(1)
