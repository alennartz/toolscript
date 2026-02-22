FROM rust:1.85-slim AS builder
WORKDIR /build
COPY . .
RUN apt-get update && apt-get install -y pkg-config libssl-dev && \
    cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/code-mcp /usr/local/bin/code-mcp
ENTRYPOINT ["code-mcp", "run"]
