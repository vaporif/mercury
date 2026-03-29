FROM lukemathwalker/cargo-chef:latest-rust-1.94.1-alpine AS chef
WORKDIR /build

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
RUN apk add --no-cache musl-dev cmake make pkgconf

COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release --locked --bin mercury-relayer

FROM alpine:3
RUN apk add --no-cache ca-certificates
COPY --from=builder /build/target/release/mercury-relayer /usr/local/bin/
ENTRYPOINT ["mercury-relayer"]
