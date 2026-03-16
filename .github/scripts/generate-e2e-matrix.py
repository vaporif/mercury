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

    if manifest_path.exists():
        with open(manifest_path, "rb") as f:
            manifest = tomllib.load(f)
    else:
        manifest = {}

    defaults = manifest.get("defaults", {})
    default_setup = defaults.get("setup", ["cosmos"])
    default_timeout = defaults.get("timeout", 10)
    default_nextest_profile = defaults.get("nextest_profile", "default")

    chain_types = manifest.get("chain_types", {})
    binaries = manifest.get("binary", {})

    try:
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
    except subprocess.CalledProcessError as e:
        print(f"Failed to list tests: {e.stderr}", file=sys.stderr)
        sys.exit(1)

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

        if not binary_id.startswith("mercury-e2e::"):
            continue

        binary_name = binary_id.removeprefix("mercury-e2e::")

        binary_config = binaries.get(binary_name, {})
        setup = binary_config.get("setup", default_setup)
        skip_tests = set(binary_config.get("skip_tests", []))

        if test_name in skip_tests:
            continue

        # Resolution order: binary > max across chain_types > defaults
        timeout = binary_config.get("timeout") or max(
            (chain_types.get(s, {}).get("timeout", default_timeout) for s in setup),
            default=default_timeout,
        )
        nextest_profile = binary_config.get("nextest_profile") or next(
            (chain_types.get(s, {}).get("nextest_profile") for s in setup
             if chain_types.get(s, {}).get("nextest_profile")),
            default_nextest_profile,
        )

        matrix.append({
            "binary": binary_name,
            "test": test_name,
            "setup": setup,
            "timeout": timeout,
            "nextest_profile": nextest_profile,
        })

    print(json.dumps(matrix))


if __name__ == "__main__":
    main()
