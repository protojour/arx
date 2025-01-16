# default target
target := "x86_64-unknown-linux-musl"

# build protojour/arx:dev debug image
dev-image:
    docker build . -t protojour/arx:dev --platform linux/amd64 --build-arg RUST_PROFILE=debug --build-arg CARGO_FLAGS=

release-image:
    docker build . -t protojour/arx:dev --platform linux/amd64 --build-arg RUST_PROFILE=release --build-arg CARGO_FLAGS=--release

