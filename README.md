# Mercury

An IBC v2 relayer in Rust. Plain traits, no frameworks.

## Status

Early development.
Currently in design/architecture phase.

## Why

The existing IBC relayer SDK ([hermes-sdk](https://github.com/informalsystems/hermes-sdk)) is built on a macro-heavy framework ([CGP](https://github.com/contextgeneric/cgp))
that makes the code difficult to contribute to.
Mercury replaces that with standard Rust traits and generics.

## Docs

- [Why not CGP?](./docs/cgp.md)
- [Architecture](./docs/architecture.md)  
- [IBC v2](./docs/ibc-v2.md)

## License

Apache-2.0
