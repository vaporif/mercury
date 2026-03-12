# Mercury

An IBC v2 relayer in Rust. Plain traits, no frameworks.

Mercury relays packets between IBC-connected blockchains. Unlike [hermes-sdk](https://github.com/informalsystems/hermes-sdk), which uses Context-Generic Programming (250+ component traits, macro-heavy abstractions, 20-minute compile times), Mercury uses plain Rust traits and generics — ~35 focused traits, readable error messages, and standard tooling that just works. If you've tried to contribute to an IBC relayer and bounced off the complexity, this is for you.

## Status

Early active development. Core IBC v2 relay pipeline is implemented but **not yet tested against live chains**. No unit or integration tests yet. Use at your own risk. Or better don't even try to use until I add unit/integration/e2e tests.

## Docs

- [Why rewrite?](./docs/why-rewrite.md)
- [Architecture](./docs/architecture.md)  
- [IBC v2](./docs/ibc-v2.md)

## Usage

```bash
# Start the relayer
mercury start --config relayer.toml

# Query chain status
mercury status --config relayer.toml --chain cosmoshub-4
```

See [`examples/relayer.toml`](./examples/relayer.toml) for a full config example.

## Development

### With Nix (recommended)

The project includes a Nix flake with a complete dev shell. If you have Nix with flakes enabled:

```bash
# Enter the dev shell (or use direnv with the included .envrc)
nix develop

# Build the binary via Nix
nix build
```

The dev shell provides the stable Rust toolchain (cargo, clippy, rustfmt, rust-analyzer), plus `cargo-nextest`, `taplo`, `typos`, and `actionlint`.

### Without Nix

Requires a stable Rust toolchain. Install via [rustup](https://rustup.rs).

### Building and testing

```bash
cargo build
cargo test
cargo clippy --workspace
```

## License

Apache-2.0
