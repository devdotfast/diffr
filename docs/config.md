# Configuration

diffr's configuration comes from exactly three places:

1. The global file, `$XDG_CONFIG_HOME/diffr/config.toml` (usually
   `~/.config/diffr/config.toml`). `--config PATH` replaces it and must exist;
   a missing global file means every default.
2. Command-line flags, for one run. `--byte-limit`, `--graph-limit` and
   `--parse-error-limit` override `[diff]`, and `-U` overrides
   `plugins.bundled.context.lines`.
3. Git attributes, for facts about files in a repository (`.gitattributes`
   and git's user-wide attributes file); see [File tags](#file-tags).

There is no repository configuration file and no environment-variable layer.
The embedded [default config](../src/config/default.toml) provides fresh-install
settings. Config files use `version = 1`; unsupported versions are rejected.
Keys the file omits keep their defaults. An unknown key or a mistyped value is
an error that names the file and the key's dotted path, for example
`config.toml: diff.typo: unknown field`.

## Commands

```sh
diffr config                  # settings screen in the terminal frontend
diffr config theme            # the same, searching for "theme"
diffr config schema           # JSON Schema: title, group, description and default per key, plugins included
diffr config show [--json]    # the resolved configuration
diffr config set diff.graph_limit 5000000
diffr config set theme.name default-light
diffr config set plugins.bundled.deleted-bodies.min_lines 20
```

On the first successful edit, `set` writes the complete resolved config,
including `version = 1`, plugin order, and option defaults. Later edits preserve
existing values and comments. Existing partial files are filled in on edit too;
explicit plugin lists remain authoritative. This pins today's defaults until
the user edits them or a future version migration changes them.

To add an external plugin, edit the config file: add its
`[plugins.external.NAME]` entry with a `path`, and include `external.NAME`
at the desired position in `plugins.order`.

`set` writes one key into the global file (or the `--config` file). The value
is read as the type the schema gives the key, and only that type: a string key takes the text as it is
(`diffr config set theme.name 1234` writes `"1234"`), an integer key a TOML
integer (`12`), a number key a TOML number (`1.5`), a boolean key `true` or
`false`, an array key a TOML array (`'["a", "b"]'`), and an enum key one of
its choices. A plugin option has the type its `plugin.toml` declares, a WASM
plugin's folder included, and an entry's `path` is a string. Anything else, an
unknown key, or a value the configuration rejects (such as a negative limit)
is an error naming the key and what it expected, for example
`diff.graph_limit: expected an integer, got "abc"`; diffr exits 2 and the file
is not touched. Frontends drive their settings pages through these three
commands; the schema is the only contract. Each setting in the schema carries
a `title` and an `x-group` that settings screens show in place of the dotted
key; keys marked `"x-settings": false` are not settings.

## Keys

```toml
version = 1

[diff]
byte_limit = 1000000       # larger files on either side get a line diff
graph_limit = 3000000      # the largest AST matching graph explored per file
parse_error_limit = 0      # more tree-sitter parse errors than this: line diff

[theme]
name = "default-dark"      # a bundled terminal theme
path = "/path/to/theme.toml"  # or a Helix-style theme file

[plugins]                  # see "Plugins" below
order = ["bundled.context", "bundled.hide-files", "bundled.deleted-bodies", "bundled.summarize", "bundled.test-bodies", "bundled.removed-runs", "bundled.group"]
```

When a file exceeds a `[diff]` limit it falls back to a line diff: the file's
`stats` carries a `fallback` with code `too_large`, `too_complex` or
`parse_error` and a message naming the key to raise. `--byte-limit`,
`--graph-limit` and `--parse-error-limit` override the file for one run. The
defaults are difftastic's. A large rewrite can exceed the graph limit and fall
back to a line diff; raising the limit trades memory for it across every file
diffed in parallel, so prefer narrowing the comparison or leaving the fallback.

## Plugins

After a file is diffed and its regions are built, plugins decide how it starts
out on screen: which files are hidden, which regions start collapsed and with
what label, which regions open and close together, and which collapsed
regions are grouped. They run in `plugins.order`, each on the region trees the
one before it left. [streaming.md](streaming.md#plugins) describes what they
produce on the wire.

```toml
[plugins]
order = ["bundled.context", "bundled.hide-files", "bundled.deleted-bodies", "bundled.summarize", "bundled.test-bodies", "bundled.removed-runs", "bundled.group"]

[plugins.bundled.context]                           # unchanged lines far from any change collapse
enabled = true
lines = 3                                   # kept on either side of a change; -U overrides it

[plugins.bundled.hide-files]
enabled = true
tags = ["generated", "vendored", "test"]   # the first listed tag a file carries names the reason
deleted = true                              # hide deleted files whatever their tags

[plugins.bundled.deleted-bodies]                    # deleted function bodies, "12 lines removed"
enabled = true
min_lines = 12

[plugins.bundled.test-bodies]                       # test functions and test modules, on both sides
enabled = true
min_lines = 3

[plugins.bundled.removed-runs]                      # the middle of long removed stretches
enabled = true
min_lines = 5

[plugins.bundled.summarize]                         # pseudocode for large new function bodies and right-side tests
enabled = false            # off by default: it needs an API key
provider = "gemini"
model = "gemini-3.8-flash"
min_lines = 20
tests = true              # include right-side tests even when not newly added
test_min_lines = 20
api_key = "…"              # or GEMINI_API_KEY / GOOGLE_API_KEY
endpoint = "https://…"     # optional base URL override
request_timeout_ms = 60000
max_concurrency = 16      # legacy option; ignored by the WASM summarizer
retries = 3
system_prompt = """…"""    # the model's system instruction; defaults to diffr's own

[plugins.bundled.group]                             # adjacent collapsed regions under one row
enabled = true
```

Without an explicit `plugins.order`, bundled defaults are available and the
file may override individual settings. With an explicit order, only the
listed bundled plugins and declared entries participate: diffr does not add
other bundled plugins. Every declared entry must appear exactly once.

Entries live in `plugins.bundled` or `plugins.external`; order uses qualified
references such as `bundled.context` and `external.mine`. These references do
not change plugin names or query tags. A bundled entry cannot set `path`.
An external entry must set `path` to a folder containing `plugin.toml` and
`plugin.wasm`, relative to the configuration file or absolute. Missing WASM
components are errors; external entries never fall back to native code.
Only one enabled implementation may use a given plugin name.

`enabled` and `path` belong to diffr. Other keys are validated against the
plugin's option schema and filled with defaults. Unknown options and wrong
types are errors naming the entry and option.

Every plugin that is on is made (its `new` runs) when diffr starts, before
any output, and one that cannot be made stops diffr with an error naming it.
The summarizer cannot be made without an API key, which is why it ships off: turn it on with
`enabled = true` once `api_key`, `GEMINI_API_KEY` or `GOOGLE_API_KEY` holds a
key.

```toml
[plugins]
order = ["bundled.context", "bundled.hide-files", "bundled.deleted-bodies", "bundled.summarize", "bundled.test-bodies", "bundled.removed-runs", "bundled.group", "external.fixtures"]

[plugins.external.fixtures]
path = "plugins/fixtures"
```

`summarize`'s `system_prompt` is the system instruction sent with every
request. The per-file message (the file's numbered lines and the folds to
summarize) is built by diffr. `diffr config show` prints the default prompt.

### Plugin folders

Every bundled plugin is a folder under `plugins/`, laid out the way any plugin
is:

```text
plugins/deleted-bodies/
  plugin.toml           # name, title, options
  queries/rust.scm      # one query file per language
  Cargo.toml, src/lib.rs  # the plugin's code, a crate using the plugin SDK
  plugin.wasm           # the same code built as a WASM component (not committed)
```

diffr embeds a bundled plugin's `plugin.toml` and query files and compiles
its code in. Any other plugin's folder holds `plugin.wasm`.

`plugin.toml` is static:

```toml
name = "deleted-bodies"               # the entry name, and the prefix of its tags
title = "Deleted function bodies"     # the group settings screens list its settings under
description = "Collapse function bodies that were deleted: …"

[enabled]                             # how settings screens show the switch
title = "Collapse deleted function bodies"
description = "…"
default = true                        # whether the plugin is on unless an entry says otherwise

[options.min_lines]                   # each option is a JSON Schema, written in TOML
type = "integer"
minimum = 0
title = "Shortest body to collapse (lines)"
description = "Deleted bodies shorter than this stay open."
default = 12
```

Every option needs a `title`; one with a `default` is pre-filled in
the plugin's bundled or external entry, and one without starts unset. Options
are listed in the settings schema in the order `plugin.toml` declares them.

### Queries

Tree-sitter queries decide which folds exist and what they are, as tags;
plugins decide how they are shown. Each plugin returns named source text from
`queries()`, called once on its instance during setup. Each source has a
`language` (a lowercase language name such as `rust` or `typescripttsx`),
`name` for imports and diagnostics, and `text`. Rust plugins can embed their
`.scm` files with `include_str!`; no `[queries]` section is used in
`plugin.toml`, and queries are not configured in `[plugins]`.

For each language, diffr concatenates the sources of every enabled plugin,
in `order`, into one compiled and validated query before the stream starts.
A source whose first line is `; inherits: shared.scm` (several names separated
by commas) includes its dependencies first. Relative imports resolve against
the importing source's name. Return those sources from `queries()` too;
source names should include the plugin name to avoid accidental collisions.
Returned sources take precedence over files. An unresolved `builtin:` import
loads a bundled file, and an absolute-path import loads a file on disk.
Relative imports from an absolute source name resolve beside that file.
Each source is included at most once per language; import cycles and different
text returned under the same name are setup errors.

The structure every bundled plugin shares (blocks, collections, imports,
strings) lives in `plugins/shared/queries/<language>.scm`, which is not a
plugin: the bundled queries import it as
`builtin:shared/queries/<language>.scm`.

Docstrings live beside it, in `plugins/shared/queries/<language>-docstrings.scm`:
comments directly above a function (Rust `///` and `//` runs and `/** */`
doc comments, Go comment groups, JavaScript and TypeScript comments and JSDoc)
and Python's leading string. The plugins that collapse function bodies
(`deleted-bodies`, `test-bodies`, `summarize`) import it, and each links a
body it collapses to its docstring, so the two open and close together.
Because a file is included once however many plugins import it, each
docstring pattern sets one tag per importer (`deleted-bodies:docstring`,
`test-bodies:docstring`, `summarize:docstring`), and a docstring carries all
three whenever any of those plugins is enabled. A docstring is tagged only
when its function's body spans lines: a comment above a one-line function or
a plain declaration still folds, untagged.

`src/config/README.md` describes the capture conventions (`@fold`,
`@fold.open`, `@fold.close` and `#set! tag`). Every `@fold` capture in one match
forms one fold, so a quantified run such as `(comment)+ @fold` folds as one
region spanning the run. Tags name a plugin and are written in the query:
`(#set! tag "deleted-bodies:function")`. A tag must have the form
`<plugin>:<name>` with a plugin from `plugins.order`, or the configuration
does not compile; a plugin reads only the tags its own queries set. A
pattern may set no tag at all, to make a fold exist without meaning anything
to a plugin.

Patterns from different files that capture the same syntax node merge into
one fold with the union of their tags when they agree on its range. When they
capture it with different ranges, that file is not diffed: its record carries
an error with code `query_conflict` naming both files, for example
`src/lib.rs:42: builtin:deleted-bodies/queries/rust.scm and
builtin:summarize/queries/rust.scm capture the same function_item with
different fold ranges`. Other files proceed.

A query file that does not compile (a syntax error, an unknown node, an
unsupported capture or directive, a malformed tag) is an error naming the
file, and diffr exits 2 before the stream starts.

### Settings screen

`diffr config schema` builds the `plugins` part of the schema from each
plugin's `plugin.toml`. Every scalar setting under `plugins` has a `title`
and an `x-group`: the group is the plugin's `title` (`Context`, `Hidden files`,
`Deleted function bodies`, `Test bodies`, `Removed stretches`,
`Summaries`, `Groups`), and each `enabled` is titled after what that plugin
does. `plugins.order`, every list option such as `hide-files.tags`, and
`summarize.system_prompt` (a multi-line value) are marked `"x-settings":
false`: the schema describes them, a settings screen does not edit them, and
they are set in the file or with `diffr config set`. `diffr config show` redacts `plugins.bundled.summarize.api_key` unless
`--reveal` is given.

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
