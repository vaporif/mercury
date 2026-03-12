FROM rust:1.94-alpine AS builder

RUN apk add --no-cache musl-dev cmake make pkgconf

WORKDIR /build

COPY . .

RUN cargo build --release --locked

FROM alpine:3

RUN apk add --no-cache ca-certificates

COPY --from=builder /build/target/release/mercury /usr/local/bin/mercury

ENTRYPOINT ["mercury"]
