# Syntax annotation configuration

The caller parses TOML with `Config::from_toml`, builds the plugin pipeline, then calls `compile_with(&pipeline)` once.
The resulting `Params` owns one compiled query per language, holding the enabled plugins' fold
patterns, and is borrowed by each diff.
`Config::load` reads the global file (`$XDG_CONFIG_HOME/diffr/config.toml`) or an explicit replacement file.
File selection and ordering belong to the caller, not this configuration.

```rust
let config = Config::from_toml(toml_source)?;
let pipeline = Pipeline::from_config(&config.plugins, workdir)?;
let params = config.compile_with(&pipeline)?;
let result = DiffResult::from_sources_with_params(path, before, after, &params);
```

Fold query sources are owned by plugin code. `Plugin::queries()` returns
`QuerySource { language, name, text }` records, typically embedding `.scm`
files with `include_str!`. `compile_with()` collects the enabled instances'
sources and concatenates them into one validated query per language,
remembering which source each pattern came from (see `src/plugin/queries.rs`
and [docs/config.md](../../docs/config.md#queries)). Shared `inherits`
imports remain supported; `plugin.toml` contains only metadata and options.

```scheme
; inherits: builtin:shared/queries/rust.scm
((function_item body: (block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "deleted-bodies:function"))
```

The `context` plugin's files are fold queries like any other: they tag
the folds of the scopes whose first and last line stay visible around a
change (see [Scopes](#scopes)).

Language keys are lowercase built-in language names, such as `rust`,
`python`, `go`, `javascript`, and `typescript`.

## Folds and tags

`@fold` selects a node's full source range. To fold an interior instead,
capture its delimiters as `@fold.open` and `@fold.close`. The hidden range runs
from the opening node's end to the closing node's start. A fold covers only
the lines it holds whole: code before it on the line it opens on, or after it
on the line it closes on, belongs to the leaf beside it, so collapsing a fold
hides its lines and nothing else. These are direct
Tree-sitter coordinates; labels and comments before the opening brace stay visible.
With both delimiter captures, they must be ordered and contained in the fold node.
A `@fold.open` without `@fold.close` hides from the opening node's end to the end
of the fold node; the opening node may precede the fold node, so
`(function_definition ":" @fold.open body: (block) @fold)` folds a Python body
from its header's `:`. A `@fold.close` without `@fold.open` selects no fold.

These fold-boundary captures are our convention. There are no arbitrary byte or
line offsets, and no `#offset!` or `#make-range!` directives.

Every `@fold` capture in one match forms one fold, attached to the first
captured node: a quantified run such as `((comment)+ @fold . (function_item))`
folds as one region over the hull of the run. Delimiter captures apply only to
a match with a single `@fold`.

`#set! tag "<plugin>:<name>"` supplies metadata for one plugin. The prefix
must name a plugin in `plugins.order`; `compile()` rejects any other tag. Dots
in the name have no special meaning. Rules selecting the same node and range
accumulate sorted, unique tags, whichever files they came from. A node whose
rules select different ranges is a query conflict: the file is not diffed, and
its record carries a `query_conflict` error naming both query files, so the
result never depends on query order. Tags do not decide what starts
collapsed; plugins do, reading only regions, their tags and the text.

A node has at most one fold, so two folds align exactly when the matcher
paired their nodes. When a node that has a fold is flattened into its only
child during conversion (a Python `block` holding one statement, say), the
fold moves onto the child; a child that has a fold of its own keeps it, and
the wrapper node stays so that each fold still has a node.

## Scopes

The `context` plugin reads scopes from its own tags. A fold tagged
`context:scope` is a construct whose first and last line stay visible when a
change falls inside it. The tag goes on the construct's own node, so the
scope runs from the line its signature starts on to the line that closes it,
however many lines the signature takes. The body fold the shared query gives
the same construct covers the body alone and starts a line later, so the two
are different regions: one fold per node still holds, and the scope nests
around the body.

```scheme
; inherits: builtin:shared/queries/rust.scm
((function_item) @fold
  (#set! tag "context:scope"))
```

## Supported query features

Tree-sitter supplies query syntax and text predicates such as `#eq?` and `#match?`.
This module implements `#set!` with the `tag` property. It does not implement arbitrary Neovim directives or Lua callbacks.
Helper captures must begin with `_`. Unsupported captures, properties, directives,
invalid TOML, and invalid query syntax fail during `compile()`, naming the query file.

## Resolved language configuration

There are two inputs to language configuration:

- **How to parse a file:** [`guess_language.rs`](../parse/guess_language.rs)
  detects the language; [`TreeSitterConfig` / `build_config`](../parse/tree_sitter_parser.rs)
  supplies its built-in Tree-sitter grammar and parsing/highlighting rules.
- **What syntax to expose in a diff:** [`Config`](../config.rs) supplies
  the enabled plugins' queries, which identify foldable AST regions and
  their tags. The bundled queries live
  in [`plugins/<name>/queries/`](../../plugins/).

[`Params::language`](../config.rs) resolves both into one `LanguageParams`:
the parser configuration plus the assembled query compiled against its grammar.
Parsing receives only this entry, including resolved embedded-language entries.
It does not also receive the server-wide `Params`. Shared entries reuse the compiled
queries when a language appears both directly and inside another language.

Every supported language has an entry, even without annotation rules. Those
languages retain structural diffing and highlighting with empty annotation queries;
their grammar is initialized on first use. Custom queries are validated during
`Config::compile`. Files without a supported language still use textual diffing.
