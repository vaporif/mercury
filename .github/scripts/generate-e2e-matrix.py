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

    data = json.loads(result.stdout)
    suites = data.get("rust-suites", {})

    matrix = []

    for binary_id, suite in suites.items():
        if not binary_id.startswith("mercury-e2e::"):
            continue

        binary_name = binary_id.removeprefix("mercury-e2e::")
        testcases = suite.get("testcases", {})

        binary_config = binaries.get(binary_name, {})
        setup = binary_config.get("setup", default_setup)
        skip_tests = set(binary_config.get("skip_tests", []))

        # Resolution order: binary > max across chain_types > defaults
        binary_timeout = binary_config.get("timeout")
        timeout = binary_timeout if binary_timeout is not None else max(
            (chain_types.get(s, {}).get("timeout", default_timeout) for s in setup),
            default=default_timeout,
        )
        for test_name in testcases:
            # test_name is module-qualified (e.g. "transfer::ibc_transfer")
            bare_name = test_name.rsplit("::", 1)[-1]

            if bare_name in skip_tests:
                continue

            matrix.append({
                "binary": binary_name,
                "test": test_name,
                "setup": setup,
                "timeout": timeout,
            })

    print(json.dumps(matrix))


if __name__ == "__main__":
    main()
