FROM --platform=$BUILDPLATFORM ghcr.io/rust-cross/rust-musl-cross:x86_64-musl AS cross_amd64
FROM --platform=$BUILDPLATFORM ghcr.io/rust-cross/rust-musl-cross:aarch64-musl AS cross_arm64


FROM cross_${TARGETARCH} AS cross
ARG TARGETARCH
RUN apt-get update && apt-get install --no-install-recommends -y protobuf-compiler=3.12.4-1ubuntu7.22.04.1
RUN cargo install cargo-chef --target x86_64-unknown-linux-gnu
WORKDIR /app


FROM cross AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json


FROM cross AS builder_amd64
ARG CARGO_FLAGS
# Build dependencies:
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=ssh cargo chef cook ${CARGO_FLAGS} --target x86_64-unknown-linux-musl --recipe-path recipe.json
# Build application:
COPY . .
RUN cargo build -p arx ${CARGO_FLAGS} --target x86_64-unknown-linux-musl

FROM cross AS builder_arm64
ARG CARGO_FLAGS
# Build dependencies:
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=ssh cargo chef cook ${CARGO_FLAGS} --target aarch64-unknown-linux-musl --recipe-path recipe.json
# Build application:
COPY . .
RUN cargo build -p arx ${CARGO_FLAGS} --target x86_64-unknown-linux-musl


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
