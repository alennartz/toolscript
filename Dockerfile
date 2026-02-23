FROM rust:1.88-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y \
        musl-tools musl-dev g++ pkg-config \
    && rustup target add x86_64-unknown-linux-musl \
    && rm -rf /var/lib/apt/lists/*

# Provide the glibc symbol that libstdc++.a references but musl lacks.
RUN echo 'char __libc_single_threaded = 0;' | \
    musl-gcc -c -x c - -o /tmp/single_threaded.o && \
    ar rcs /usr/local/lib/libsingle_threaded.a /tmp/single_threaded.o

ENV CC_x86_64_unknown_linux_musl=musl-gcc \
    CXX_x86_64_unknown_linux_musl=g++ \
    AR_x86_64_unknown_linux_musl=ar \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
    RUSTFLAGS="-C target-feature=+crt-static -C relocation-model=static -L /usr/local/lib -l static=single_threaded"
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM scratch
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /tmp /tmp
COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/code-mcp /code-mcp
ENTRYPOINT ["/code-mcp", "run"]
