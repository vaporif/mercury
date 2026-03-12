# Mercury

An IBC v2 relayer in Rust. Plain traits, no frameworks.

Mercury relays packets between IBC-connected blockchains. Unlike [hermes-sdk](https://github.com/informalsystems/hermes-sdk), which uses Context-Generic Programming (250+ component traits, macro-heavy abstractions, 30-minute compile times), Mercury uses plain Rust traits and generics — ~35 focused traits, readable error messages, and standard tooling that just works. If you've tried to contribute to an IBC relayer and bounced off the complexity, this is for you.

## Status

Early development.
Active development phase.

## Docs

- [Why rewrite?](./docs/why-rewrite.md)
- [Architecture](./docs/architecture.md)  
- [IBC v2](./docs/ibc-v2.md)

## Building

```bash
cargo build
cargo test
cargo clippy --workspace
```

## License

Apache-2.0
