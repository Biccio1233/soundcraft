# Linux build/CI image for WaveCraft work in this workspace: the headless `wavecraft-cli` and the
# full `cargo xtask ci` gate. The desktop apps stay native; sources are bind-mounted, not copied.
# cmake and clang build whisper.cpp (whisper-rs-sys) and libclang feeds bindgen; ONNX Runtime comes
# prebuilt through `ort`. Models are never in the image: they download on first use.
# Trixie (GCC 14): ort's prebuilt ONNX Runtime links against a newer libstdc++ than bookworm's.
FROM rust:1.99-trixie
RUN apt-get update \
 && apt-get install -y --no-install-recommends libasound2-dev pkg-config git ca-certificates \
      mesa-vulkan-drivers libvulkan1 \
      cmake clang libclang-dev \
 && rm -rf /var/lib/apt/lists/*
RUN rustup component add rustfmt clippy && rustup target add wasm32-unknown-unknown
ENV CARGO_TARGET_DIR=/target
# whisper.cpp's ggml builds for the generic CPU: `-mcpu=native` inside Docker on Apple silicon makes
# GCC 12 refuse its fp16 intrinsics ("target specific option mismatch").
ENV GGML_NATIVE=OFF
WORKDIR /src
