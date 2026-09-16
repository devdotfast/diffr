; Docstrings, imported by the plugins that collapse function bodies: each
; links a body it collapses to the docstring tagged with its own name (see
; `docstring_of` in crates/diffr-plugin-sdk/src/tree.rs). A file is included once however many
; plugins import it, so a docstring carries one tag per importer.
;
; A docstring: the run of comments directly above a function. It is tagged
; only when the function's body spans lines, and so is a region: the fold
; just before a body is then never the docstring of a one-line function
; above it.
((comment)+ @fold . [(function_declaration) (method_declaration)])
((comment)+ @fold
  .
  [
    (function_declaration body: (block) @_body)
    (method_declaration body: (block) @_body)
  ]
  (#match? @_body "\n")
  (#set! tag "deleted-bodies:docstring")
  (#set! tag "summarize:docstring")
  (#set! tag "test-bodies:docstring"))
