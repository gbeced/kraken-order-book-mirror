# Linux image for running the gungraun benches (Valgrind is Linux-only).
# Used by scripts/bench.sh.
FROM ubuntu:24.04

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl pkg-config libssl-dev valgrind libc6-dbg \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain 1.99

# Must match the `gungraun` version in Cargo.toml.
ARG GUNGRAUN_VERSION=0.20.0
RUN cargo install --locked gungraun-runner --version "${GUNGRAUN_VERSION}"

WORKDIR /work
