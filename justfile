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
build-solana-fixtures force="true":
    #!/usr/bin/env bash
    set -euo pipefail
    SRC=external/solidity-ibc-eureka/programs/solana
    OUT=target/solana-fixtures
    PROGS=(ics26_router ics07_tendermint access_manager test_ibc_app)
    if [ "{{force}}" != "true" ]; then
        all_present=true
        for prog in "${PROGS[@]}"; do
            [ -f "$OUT/${prog}.so" ] && [ -f "$OUT/${prog}-keypair.json" ] || { all_present=false; break; }
        done
        if $all_present; then
            echo "Solana fixtures already present at $OUT — skipping build"
            exit 0
        fi
    fi
    mkdir -p "$OUT"
    (cd "$SRC" && anchor build)
    for prog in "${PROGS[@]}"; do
        cp "$SRC/target/sbf-solana-solana/release/${prog}.so" "$OUT/${prog}.so"
        cp "external/solidity-ibc-eureka/solana-keypairs/localnet/${prog}-keypair.json" "$OUT/${prog}-keypair.json"
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
