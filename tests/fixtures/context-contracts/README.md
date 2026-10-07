# Context contracts

Each language file contains the source pair, diff action and expected visible
rows. `index.json` maps cases to the reviewed decisions.

```sh
cargo test --test context_contracts
```

Assertions use `diffr pprint` with one context line and stable relative file paths.
Expected output includes file headers, base/head line numbers, fold IDs and the footer.

`RunStartingAt` and `BodyOf` use one-based lines in the after source.
Each action opens only the selected fold; nested folds keep their own state.
The JSX and TSX cases show a collapsed sibling run, its opened component
outlines, and an opened component body.
