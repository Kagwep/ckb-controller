# Toolchain image for building the CKB contracts WITHOUT a local riscv/clang
# setup — the `ckb-controller build --docker` path.
#
#   docker build -t ckb-controller-toolchain -f docker/build.Dockerfile .
#   node cli/bin.mjs build --docker
#
# (Docker was not available on the machine this was authored on, so treat this
# image as a starting point: `build.sh` itself is the contract — a bash, rustup
# with the riscv64imac target, and any clang 16+ for ckb-std's C stub.)
FROM rust:1.85.1-slim-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
    clang \
    make \
    bash \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add riscv64imac-unknown-none-elf

WORKDIR /work
CMD ["bash", "./build.sh"]
