# Linux build/CI image for WaveCraft work in this workspace: the headless `wavecraft-cli` and the
# full `cargo xtask ci` gate. The desktop apps stay native; sources are bind-mounted, not copied.
FROM rust:1.99-bookworm
RUN apt-get update \
 && apt-get install -y --no-install-recommends libasound2-dev pkg-config git ca-certificates \
      mesa-vulkan-drivers libvulkan1 \
 && rm -rf /var/lib/apt/lists/*
RUN rustup component add rustfmt clippy && rustup target add wasm32-unknown-unknown
ENV CARGO_TARGET_DIR=/target
WORKDIR /src
