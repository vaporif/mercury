# Run all checks
check: check-typos check-toml fmt clippy test

# Check for typos
check-typos:
    typos

# Lint TOML files
check-toml:
    taplo check

# Format check
fmt:
    cargo fmt --all -- --check

# Clippy
clippy:
    cargo clippy --workspace -- -D warnings

# Run tests
test:
    cargo nextest run --workspace

# Run e2e tests
e2e:
    cargo nextest run -p mercury-e2e --run-ignored all

# Build Solana programs from the eureka submodule into target/solana-fixtures/
build-solana-fixtures:
    #!/usr/bin/env bash
    set -euo pipefail
    SRC=external/solidity-ibc-eureka/programs/solana
    OUT=target/solana-fixtures
    mkdir -p "$OUT"
    (cd "$SRC" && anchor build)
    for prog in ics26_router ics07_tendermint access_manager test_ibc_app; do
        cp "$SRC/target/deploy/${prog}.so" "$OUT/${prog}.so"
        cp "$SRC/target/deploy/${prog}-keypair.json" "$OUT/${prog}-keypair.json"
    done
    echo "Solana fixtures staged at $OUT"

# Run the cosmos-solana e2e test end-to-end (builds fixtures automatically if missing)
e2e-cosmos-solana:
    cargo test -p mercury-e2e --test cosmos_solana -- --ignored --nocapture

# Format everything
fmt-fix:
    cargo fmt --all
    taplo fmt
    nix fmt -- flake.nix
