FROM rust:1.85-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends python3 cmake clang pkg-config && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_TOOLCHAIN=1.85.0 CARGO_BUILD_JOBS=4
WORKDIR /work
COPY . .
RUN --mount=type=cache,target=/work/target,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    target=$(rustc -vV | sed -n 's/^host: //p') && \
    version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1) && \
    cargo build --release --locked && \
    mkdir -p /lean/bin /full/bin /artifacts && cp target/release/diffr /lean/bin/ && \
    cargo build --release --locked --features all-languages && cp target/release/diffr /full/bin/ && \
    python3 scripts/release.py pack --version "$version" --target "$target" --install /lean --cli-only --output /artifacts && \
    python3 scripts/release.py pack --version "$version" --target "$target" --install /full --edition full --output /artifacts

FROM scratch AS artifacts
COPY --from=build /artifacts/ /

FROM node:22-bookworm-slim AS runtime
WORKDIR /opt/diffr
COPY dist/ dist/
COPY diffr-ts/bin/ diffr-ts/bin/
COPY diffr-ts/package.json diffr-ts/package.json
COPY scripts/smoke_editions.mjs scripts/smoke_editions.mjs
COPY sample_files/ sample_files/
CMD ["node", "scripts/smoke_editions.mjs", "dist", "sample_files"]
