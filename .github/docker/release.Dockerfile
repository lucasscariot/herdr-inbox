# BuildKit sends the source into the daemon, so this also works when the
# Actions runner is a container with the host Docker socket mounted.
# Keep the compiler and linker native. Emulated x86 GCC collect2 can segfault.
FROM --platform=$BUILDPLATFORM rust:1-alpine AS build
ARG RUST_TARGET
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS
RUN apk add --no-cache musl-dev \
    && rustup target add "$RUST_TARGET" \
    && host=$(rustc -vV | awk '/^host:/ {print $2}') \
    && ln -s "$(rustc --print sysroot)/lib/rustlib/$host/bin/rust-lld" /usr/local/bin/rust-lld
WORKDIR /src
COPY Cargo.toml Cargo.lock install.sh ./
COPY src ./src
# Only the final binary needs the cross-linker; host proc macros keep native cc.
RUN cargo rustc --release --locked --bin herdr-inbox --target "$RUST_TARGET" -- -C linker=rust-lld \
    && mkdir /dist \
    && cp "target/$RUST_TARGET/release/herdr-inbox" /dist/ \
    && grep -m1 '^version' Cargo.toml | cut -d '"' -f 2 > /dist/version

# Execute only the completed binary on the requested platform, not the compiler.
FROM alpine AS verify
ARG TARGETARCH
COPY --from=build /dist/ /dist/
RUN case "$TARGETARCH" in amd64) machine=3e00 ;; arm64) machine=b700 ;; *) exit 1 ;; esac \
    && test "$(od -An -tx1 -N6 /dist/herdr-inbox | tr -d ' \n')" = 7f454c460201 \
    && test "$(od -An -tx1 -j18 -N2 /dist/herdr-inbox | tr -d ' \n')" = "$machine" \
    && test "$(/dist/herdr-inbox --version)" = "herdr-inbox $(cat /dist/version)"

FROM scratch AS binary
COPY --from=verify /dist/herdr-inbox /herdr-inbox
