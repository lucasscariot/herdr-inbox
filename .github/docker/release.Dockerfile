# BuildKit sends the source into the daemon, so this also works when the
# Actions runner is a container with the host Docker socket mounted.
FROM rust:1-alpine AS build
ARG RUST_TARGET
ARG CARGO_BUILD_JOBS=4
ENV CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock install.sh ./
COPY src ./src
RUN cargo build --release --locked --target "$RUST_TARGET" \
    && version=$(grep -m1 '^version' Cargo.toml | cut -d '"' -f 2) \
    && test "$(target/$RUST_TARGET/release/herdr-inbox --version)" = "herdr-inbox $version" \
    && mkdir /dist \
    && cp "target/$RUST_TARGET/release/herdr-inbox" /dist/

FROM scratch AS binary
COPY --from=build /dist/herdr-inbox /herdr-inbox
