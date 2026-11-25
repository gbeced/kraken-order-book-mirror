#!/usr/bin/env bash
# Runs `cargo bench` (gungraun + Valgrind) in a Linux container.
# Arguments are passed through to `cargo bench`, e.g.:
#   scripts/bench.sh -- --save-baseline=master
#   scripts/bench.sh --bench mirror -- --baseline=master
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
image="kraken-obm-bench"
version="$(sed -n 's/^gungraun = "\(.*\)"/\1/p' "$root/Cargo.toml")"

docker build --quiet \
    --build-arg "GUNGRAUN_VERSION=${version}" \
    -t "$image" -f "$root/docker/bench.Dockerfile" "$root/docker" >/dev/null

# Only the bench targets, unless a target is picked: the lib's test harness
# rejects gungraun flags such as --save-baseline.
targets=(--bench '*')
for arg in "$@"; do
    [ "$arg" = "--" ] && break
    case "$arg" in --bench | --bench=*) targets=() ;; esac
done

# Linux artifacts and baselines live in target/linux, apart from the host build.
# seccomp=unconfined lets gungraun disable ASLR (setarch -R) for repeatable runs.
exec docker run --rm -t \
    --security-opt seccomp=unconfined \
    -v "$root:/work" \
    -v kraken-obm-cargo-registry:/usr/local/cargo/registry \
    -e CARGO_TARGET_DIR=/work/target/linux \
    "$image" cargo bench ${targets[@]+"${targets[@]}"} "$@"
