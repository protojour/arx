FROM --platform=$BUILDPLATFORM ghcr.io/rust-cross/rust-musl-cross:x86_64-musl AS cross_amd64
FROM --platform=$BUILDPLATFORM ghcr.io/rust-cross/rust-musl-cross:aarch64-musl AS cross_arm64

FROM cross_${TARGETARCH} AS builder_base
ARG TARGETARCH
RUN apt-get update && apt-get install --no-install-recommends -y protobuf-compiler=3.12.4-1ubuntu7.22.04.1
WORKDIR /app
COPY . /app/

FROM builder_base AS builder_amd64
ARG CARGO_FLAGS
RUN cargo build -p arx ${CARGO_FLAGS} --target x86_64-unknown-linux-musl

FROM builder_base AS builder_arm64
ARG CARGO_FLAGS
RUN cargo build -p arx ${CARGO_FLAGS} --target aarch64-unknown-linux-musl

FROM builder_${TARGETARCH} AS builder
ARG TARGETARCH

FROM scratch AS dist_base
COPY --from=builder /etc/passwd /etc/passwd
COPY LICENSE /

FROM dist_base AS dist_amd64
ARG RUST_PROFILE
COPY --from=builder /app/target/x86_64-unknown-linux-musl/${RUST_PROFILE}/arx /arx

FROM dist_base AS dist_arm64
ARG RUST_PROFILE
COPY --from=builder /app/target/aarch64-unknown-linux-musl/${RUST_PROFILE}/arx /arx

FROM dist_${TARGETARCH}
ARG TARGETARCH
ENTRYPOINT ["/arx"]
CMD ["--help"]
