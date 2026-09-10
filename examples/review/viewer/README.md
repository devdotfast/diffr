# Fixture viewer

Build the CLI, capture its streams, then serve the static files:

```sh
cargo build --locked
python3 examples/review/viewer/build.py
python3 -m http.server 4176 --bind 127.0.0.1 --directory examples/review/viewer
```

Open http://127.0.0.1:4176/. Git patches and CLI NDJSON captures are fixture data.
Re-run the builder after backend changes. No diff server is required.

[CLI stream contract](../../../docs/streaming.md).
