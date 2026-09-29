FROM rust:1.85-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends python3 cmake clang pkg-config && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_TOOLCHAIN=1.85.0 CARGO_BUILD_JOBS=4
WORKDIR /work
COPY . .
RUN --mount=type=cache,target=/work/target,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    target=$(rustc -vV | sed -n 's/^host: //p') && \
    cargo xtask build-languages --target "$target" && \
    python3 languages/tests/probe.py "target/languages/$target/package" && \
    cargo xtask package-languages --target "$target" && \
    python3 languages/tests/integration.py "target/languages/$target/catalog-entry.json" --release && \
    mkdir /artifacts && \
    cp "target/languages/$target/verification/diffr-native" /artifacts/diffr && \
    archive=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["url"].rsplit("/", 1)[1])' "target/languages/$target/catalog-entry.json") && \
    cp "target/languages/$target/$archive" /artifacts/extra.tgz && \
    cp "target/languages/$target/test-fixtures.txt" /artifacts/fixtures.txt

FROM debian:bookworm-slim
COPY --from=build /artifacts/diffr /usr/local/bin/diffr
COPY --from=build /artifacts/extra.tgz /opt/extra.tgz
COPY --from=build /artifacts/fixtures.txt /opt/fixtures.txt
COPY sample_files /fixtures
COPY languages/tests/container-smoke.sh /opt/container-smoke.sh
ENV DIFFR_PARSER_DIR=/tmp/diffr-parsers
USER 65534:65534
ENTRYPOINT ["/bin/sh", "/opt/container-smoke.sh"]
