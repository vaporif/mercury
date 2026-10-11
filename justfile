alias c := check
alias t := test
alias b := build
alias r := run
alias l := clippy
alias lf := clippy-fix
alias f := fmt-fix
alias e := e2e
alias d := deny

# List recipes
default:
    @just --list

# Run all checks
check: check-typos check-toml check-actions fmt clippy test

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

# Apply clippy suggestions
clippy-fix:
    cargo clippy --workspace --fix --allow-dirty --allow-staged -- -D warnings

# Lint GitHub Actions workflows
check-actions:
    actionlint

# Audit dependencies (licenses, advisories, bans)
deny:
    cargo deny check

# Build the workspace
build:
    cargo build --workspace

# Build the release relayer binary
build-release:
    cargo build --release --bin mercury-relayer

# Run the relayer with arbitrary args, e.g. `just run start --config cfg.toml`
run *args:
    cargo run --bin mercury-relayer -- {{args}}

# Run tests (optionally filtered, e.g. `just test gas`)
test *filter:
    cargo nextest run --workspace {{filter}}

# Coverage report as in CI (lcov.info)
coverage:
    cargo llvm-cov nextest --workspace --exclude mercury-e2e --exclude mock-wasm-eth-lc --lcov --output-path lcov.info

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
    (cd "$SRC" && anchor build --no-idl)
    for prog in "${PROGS[@]}"; do
        cp "$SRC/target/sbf-solana-solana/release/${prog}.so" "$OUT/${prog}.so"
        cp "external/solidity-ibc-eureka/solana-keypairs/localnet/${prog}-keypair.json" "$OUT/${prog}-keypair.json"
    done
    echo "Solana fixtures staged at $OUT"

# Build SP1 programs from the eureka submodule
build-sp1-programs:
    cd external/solidity-ibc-eureka/programs/sp1-programs && CARGO_PROFILE_RELEASE_TRIM_PATHS=false cargo prove build

# Build, optimize, and stage the mock WASM Ethereum light client used by e2e tests
build-mock-wasm-lc:
    rustup target add wasm32-unknown-unknown
    cargo build --target wasm32-unknown-unknown --release -p mock-wasm-eth-lc
    wasm-opt --llvm-memory-copy-fill-lowering -Os \
        target/wasm32-unknown-unknown/release/mock_wasm_eth_lc.wasm \
        -o target/wasm32-unknown-unknown/release/mock_wasm_eth_lc.optimized.wasm
    gzip -kf target/wasm32-unknown-unknown/release/mock_wasm_eth_lc.optimized.wasm
    cp target/wasm32-unknown-unknown/release/mock_wasm_eth_lc.optimized.wasm.gz \
        e2e/mock-wasm-eth-lc/mock_wasm_eth_lc.wasm.gz

# Build and archive e2e tests with nextest
e2e-archive archive="mercury-e2e.tar.zst":
    cargo nextest archive --archive-file {{archive}} --package mercury-e2e

# Print the CI e2e matrix JSON for a nextest archive
e2e-matrix archive="mercury-e2e.tar.zst" manifest="e2e-matrix.toml":
    @python3 .github/scripts/generate-e2e-matrix.py {{archive}} {{manifest}}

# Syntax-check CI Python scripts
check-python:
    python3 -m py_compile .github/scripts/generate-e2e-matrix.py

# Run the cosmos-solana e2e test end-to-end (builds fixtures automatically if missing)
e2e-cosmos-solana:
    cargo nextest run -p mercury-e2e --test cosmos_solana --run-ignored all --no-capture

# Format everything
fmt-fix:
    cargo fmt --all
    taplo fmt
    nix fmt -- flake.nix

# Run nix flake checks (same as CI "Checks" job)
nix-check:
    nix flake check

# Sync git submodules
submodules:
    git submodule update --init --recursive

# Install git hooks (lefthook)
hooks:
    lefthook install
