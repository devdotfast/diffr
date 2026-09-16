# Configuration

diffr's configuration comes from exactly three places:

1. The global file, `$XDG_CONFIG_HOME/diffr/config.toml` (usually
   `~/.config/diffr/config.toml`). `--config PATH` replaces it and must exist;
   a missing global file means every default.
2. Command-line flags, for one run. `--byte-limit`, `--graph-limit` and
   `--parse-error-limit` override `[diff]`, and `-U` sets context padding.
3. Git attributes, for facts about files in a repository (`.gitattributes`
   and git's user-wide attributes file).

There is no repository configuration file and no environment-variable layer.
Keys the file omits keep their defaults. An unknown key or a mistyped value is
an error that names the file and the key's dotted path, for example
`config.toml: diff.typo: unknown field`.

## Commands

```sh
diffr config schema           # JSON Schema: title, group, description and default per key
diffr config show [--json]    # the resolved configuration
diffr config set diff.graph_limit 5000000
diffr config set theme.name default-light
```

`set` writes one key into the global file (or the `--config` file), keeping
everything else in it as written. The value is read as the type the schema
gives the key, and only that type: a string key takes the text as it is
(`diffr config set theme.name 1234` writes `"1234"`), an integer key a TOML
integer (`12`), a number key a TOML number (`1.5`), a boolean key `true` or
`false`, an array key a TOML array (`'["a", "b"]'`), and an enum key one of
its choices. Anything else, an unknown key, or a value the configuration
rejects (such as a negative limit) is an error naming the key and what it
expected, for example `diff.graph_limit: expected an integer, got "abc"`;
diffr exits 2 and the file is not touched. Frontends drive their settings pages
through these three commands; the schema is the only contract. Each setting
in the schema carries a `title` and an `x-group` that settings screens show
in place of the dotted key.

## Keys

```toml
[diff]
byte_limit = 1000000       # larger files on either side get a line diff
graph_limit = 3000000      # the largest AST matching graph explored per file
parse_error_limit = 0      # more tree-sitter parse errors than this: line diff

[theme]
name = "default-dark"      # a bundled terminal theme
path = "/path/to/theme.toml"  # or a Helix-style theme file

[languages.rust]           # tree-sitter queries; see src/config/README.md
folds = '''...'''
context = '''...'''
```

`languages` is not in the schema, so the settings screen does not show it;
`config set` still accepts its keys.

When a file exceeds a `[diff]` limit it falls back to a line diff: the file's
`stats` carries a `fallback` with code `too_large`, `too_complex` or
`parse_error` and a message naming the key to raise. `--byte-limit`,
`--graph-limit` and `--parse-error-limit` override the file for one run. The
defaults are difftastic's. A large rewrite can exceed the graph limit and fall
back to a line diff; raising the limit trades memory for it across every file
diffed in parallel, so prefer narrowing the comparison or leaving the fallback.
