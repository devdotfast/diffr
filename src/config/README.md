# Syntax annotation configuration

The caller parses TOML with `Config::from_toml`, then calls `compile()` once.
The resulting `Params` owns separately compiled fold and context queries and is borrowed by each diff.
Each classifier runs its own traversal. Combining them is a separate optimization.
File discovery and server configuration loading are not part of this module.

```rust
let params = Config::from_toml(toml_source)?.compile()?;
let result = DiffResult::from_sources_with_params(path, before, after, &params);
```

Queries live inline in TOML:

```toml
[languages.rust]
folds = '''
((block) @fold
  (#offset! @fold 0 1 0 -1)
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

`@fold` selects a node's source range. `#offset!` adjusts its start row, start byte
column, end row, and end byte column. End coordinates are exclusive. Offsets that
leave the source, reverse the range, or split a UTF-8 character discard that match.

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
This module implements the Neovim `#offset!` directive and `#set!` with the `tag`
property. It does not implement arbitrary Neovim directives or Lua callbacks.
Helper captures must begin with `_`. Unsupported captures, properties, directives,
invalid TOML, and invalid query syntax fail during `compile()`.
