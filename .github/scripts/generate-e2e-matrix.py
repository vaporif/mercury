#!/usr/bin/env python3
"""Generate a GitHub Actions matrix JSON from a nextest archive and e2e-matrix.toml."""

import json
import subprocess
import sys
import tomllib
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <archive-path> <manifest-path>", file=sys.stderr)
        sys.exit(1)

    archive_path = sys.argv[1]
    manifest_path = Path(sys.argv[2])

    # Load manifest
    if manifest_path.exists():
        with open(manifest_path, "rb") as f:
            manifest = tomllib.load(f)
    else:
        manifest = {}

    defaults = manifest.get("defaults", {})
    default_chain_type = defaults.get("chain_type", "cosmos")
    default_timeout = defaults.get("timeout", 10)
    default_nextest_profile = defaults.get("nextest_profile", "default")

    binaries = manifest.get("binary", {})

    # Discover tests from nextest archive
    result = subprocess.run(
        [
            "cargo", "nextest", "list",
            "--archive-file", archive_path,
            "--run-ignored", "all",
            "--message-format", "json",
        ],
        capture_output=True,
        text=True,
        check=True,
    )

    matrix = []

    for line in result.stdout.splitlines():
        line = line.strip()
        if not line:
            continue

        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue

        if entry.get("type") != "test":
            continue

        binary_id = entry.get("binary_id", "")
        test_name = entry.get("name", "")

        # Filter to mercury-e2e package tests
        if not binary_id.startswith("mercury-e2e::"):
            continue

        binary_name = binary_id.removeprefix("mercury-e2e::")

        # Look up binary config
        binary_config = binaries.get(binary_name, {})
        chain_type = binary_config.get("chain_type", default_chain_type)
        timeout = binary_config.get("timeout", default_timeout)
        nextest_profile = binary_config.get("nextest_profile", default_nextest_profile)
        skip_tests = binary_config.get("skip_tests", [])

        # Skip excluded tests
        if test_name in skip_tests:
            continue

        matrix.append({
            "binary": binary_name,
            "test": test_name,
            "chain_type": chain_type,
            "timeout": timeout,
            "nextest_profile": nextest_profile,
        })

    print(json.dumps(matrix))


if __name__ == "__main__":
    main()
