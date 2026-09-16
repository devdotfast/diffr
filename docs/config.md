# Configuration

diffr's configuration comes from exactly three places:

1. The global file, `$XDG_CONFIG_HOME/diffr/config.toml` (usually
   `~/.config/diffr/config.toml`). `--config PATH` replaces it and must exist;
   a missing global file means every default.
2. Command-line flags, for one run. `--byte-limit`, `--graph-limit` and
   `--parse-error-limit` override `[diff]`, and `-U` sets context padding.
3. Git attributes, for facts about files in a repository (`.gitattributes`
   and git's user-wide attributes file); see [File tags](#file-tags).

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

## File tags

Every file in a repository comparison carries `tags` in the stream manifest:
what the file is, such as `generated`, `vendored`, `docs` or `test`. There is
no configuration key for them. In order of precedence, lowest first:

1. Bundled rules. `generated`, `vendored` and `docs` follow GitHub Linguist:
   `generated` ports `lib/linguist/generated.rb` (lockfiles and other names
   and paths, headers such as `// Code generated ... DO NOT EDIT.` in a
   file's first lines, and minified JavaScript and CSS by average line
   length), and `vendored` and `docs` match the path against Linguist's
   `vendor.yml` and `documentation.yml`, bundled under `src/tags/linguist/`
   with Linguist's MIT license. `test` comes from diffr's own path rules:
   `tests/`, `test/`, `__tests__/`, `spec/`, `*_test.go`, `*.test.*`,
   `*.spec.*`, `test_*.py`, `*_test.py`, `conftest.py`, `tests.rs`, `test.rs`,
   `*_test.rs`, `*_tests.rs`. A file carries every tag that matches.
2. Git attributes, with git's own precedence between `$GIT_DIR/info/attributes`,
   `.gitattributes` files and the user-wide file (`core.attributesFile`,
   default `~/.config/git/attributes`). Attributes always beat the bundled
   rules. `linguist-generated`, `linguist-vendored` and
   `linguist-documentation` add `generated`, `vendored` and `docs` when set,
   and remove them when unset or `false`, whatever the rules said.
   `diffr-tags=a,b` adds tags: lowercase letters, digits, `-` and `_`,
   separated by commas. A malformed value is an error naming the path, and
   diffr exits 2 before the stream starts.

```gitattributes
Cargo.lock        linguist-generated=false
web/schema.json   linguist-generated diffr-tags=schema
fixtures/**       diffr-tags=fixture
```

Header and minified checks read only the first 8 KiB of a file's after side
(its before side when deleted), and only when no name, path or attribute
has already decided `generated`. A file tagged `generated` always gets a line
diff without parsing, and its `stats.fallback` has code `generated`.
Comparisons with `--no-index` are outside any repository and carry no tags.
