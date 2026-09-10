# Syntax annotation configuration

The caller parses TOML with `Config::from_toml`, then calls `compile()` once.
The resulting `Params` owns separately compiled fold and context queries and is borrowed by each diff.
Each classifier runs its own traversal. Combining them is a separate optimization.
`Config::load` reads repository-root `diffr.toml` or an explicit replacement file.
File selection and ordering belong to the caller, not this configuration.

```rust
let params = Config::from_toml(toml_source)?.compile()?;
let result = DiffResult::from_sources_with_params(path, before, after, &params);
```

Queries live inline in TOML:

```toml
[languages.rust]
folds = '''
((block "{" @fold.open "}" @fold.close) @fold
  (#set! tag "body"))
'''
context = '''
(function_item body: (block
  "{" @context.final
  "}" @context.last)) @context
'''
```

A supplied feature replaces its bundled query. Omitted features retain defaults;
an empty string disables the feature. Language keys are lowercase built-in language
names, such as `rust`, `python`, `go`, `javascript`, and `typescript`.
See `defaults.toml` for the complete bundled rules.

## Folds and tags

`@fold` selects a node's full source range. To fold an interior instead,
capture its delimiters as `@fold.open` and `@fold.close`. The hidden range runs
from the opening node's end to the closing node's start. These are direct
Tree-sitter coordinates; labels and comments before the opening brace stay visible.
Both delimiter captures must be present, ordered, and contained in the fold node.

These fold-boundary captures are our convention. There are no arbitrary byte or
line offsets, and no `#offset!` or `#make-range!` directives.

`#set! tag "name"` supplies application metadata. Tag names are arbitrary strings;
dots have no special meaning. Multiple rules selecting the same node and range
accumulate sorted, unique tags. A node whose rules select conflicting ranges is
omitted rather than choosing a rule by execution order. Tags do not control context
selection or initial UI collapse state.

## Context

These capture meanings follow `nvim-treesitter-context`:

- `@context`: the enclosing construct; its first line is the default header.
- `@context.start`: the start of the header.
- `@context.end`: the exclusive end of the header, at this capture's start.
- `@context.final`: an alternative header end, at this capture's end.

`@context.last` is our extension: also retain the capture's final occupied line,
for example a closing brace. This does not retain all the lines between the header
and closing line. The diff engine selects relevant enclosing context for changed
hunks. It does not implement Neovim's sticky-header UI.

## Supported query features

Tree-sitter supplies query syntax and text predicates such as `#eq?` and `#match?`.
This module implements `#set!` with the `tag` property. It does not implement arbitrary Neovim directives or Lua callbacks.
Helper captures must begin with `_`. Unsupported captures, properties, directives,
invalid TOML, and invalid query syntax fail during `compile()`.

## Resolved language configuration

There are two inputs to language configuration:

- **How to parse a file:** [`guess_language.rs`](../parse/guess_language.rs)
  detects the language; [`TreeSitterConfig` / `build_config`](../parse/tree_sitter_parser.rs)
  supplies its built-in Tree-sitter grammar and parsing/highlighting rules.
- **What syntax to expose in a diff:** [`Config`](../config.rs) supplies
  queries identifying foldable AST regions, their tags, and useful extra context.
  Bundled rules live in [`defaults.toml`](defaults.toml).

[`Params::language`](../config.rs) resolves both into one `LanguageParams`:
the parser configuration plus fold and context queries compiled against its grammar.
Parsing receives only this entry, including resolved embedded-language entries.
It does not also receive the server-wide `Params`. Shared entries reuse the compiled
queries when a language appears both directly and inside another language.

Every supported language has an entry, even without annotation rules. Those
languages retain structural diffing and highlighting with empty annotation queries;
their grammar is initialized on first use. Custom queries are validated during
`Config::compile`. Files without a supported language still use textual diffing.
