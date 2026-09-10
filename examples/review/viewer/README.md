# Streaming fixture viewer

Build fixtures and start the local streaming server:

```sh
cargo build --locked
python3 examples/review/viewer/build.py
target/debug/diffr server --repo examples/review/viewer/data/workspace \
  --web-root examples/review/viewer --listen 127.0.0.1:4176
```

Open http://127.0.0.1:4176/. The baseline Git patches are fixture data;
the domain result, syntax highlighting positions and fold ranges come from live
`POST /diff` requests. Navigation aborts the previous stream.

[Response contract](../../../docs/streaming.md).
