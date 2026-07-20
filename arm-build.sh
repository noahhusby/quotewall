docker run --rm -t \
  -v "$PWD":/work -w /work \
  --platform linux/arm64 \
  debian:bullseye \
  bash -lc "
    apt-get update &&
    apt-get install -y curl build-essential pkg-config libudev-dev &&
    curl https://sh.rustup.rs -sSf | sh -s -- -y &&
    source /root/.cargo/env &&
    cargo build --release
  "