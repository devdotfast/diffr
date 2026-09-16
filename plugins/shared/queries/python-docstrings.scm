; Docstrings, imported by the plugins that collapse function bodies: each
; links a body it collapses to the docstring tagged with its own name (see
; `docstring_of` in crates/diffr-plugin-sdk/src/tree.rs). A file is included once however many
; plugins import it, so a docstring carries one tag per importer.
;
; A docstring: the string that is a function body's first statement.
((function_definition body: (block . (expression_statement (string) @fold)))
  (#set! tag "deleted-bodies:docstring")
  (#set! tag "test-bodies:docstring"))
