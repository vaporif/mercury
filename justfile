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

# Format everything
fmt-fix:
    cargo fmt --all
    taplo fmt
    nix fmt -- flake.nix
