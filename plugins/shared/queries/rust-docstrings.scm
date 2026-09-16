; Docstrings, imported by the plugins that collapse function bodies: each
; links a body it collapses to the docstring tagged with its own name (see
; `docstring_of` in crates/diffr-plugin-sdk/src/tree.rs). A file is included once however many
; plugins import it, so a docstring carries one tag per importer.
;
; A docstring: the run of doc comments among a function's attributes, or the
; run of line comments directly above it. It is tagged only when the
; function's body spans lines, and so is a region: the fold just before a
; body is then never the docstring of a one-line function above it.
(function_item (attributes (line_outer_doc_comment)+ @fold))
(function_item (attributes (block_outer_doc_comment) @fold))
((line_comment)+ @fold . (function_item))
((function_item
   (attributes (line_outer_doc_comment)+ @fold)
   body: (block) @_body)
  (#match? @_body "\n")
  (#set! tag "deleted-bodies:docstring")
  (#set! tag "test-bodies:docstring"))
((function_item
   (attributes (block_outer_doc_comment) @fold)
   body: (block) @_body)
  (#match? @_body "\n")
  (#set! tag "deleted-bodies:docstring")
  (#set! tag "test-bodies:docstring"))
((line_comment)+ @fold . (function_item body: (block) @_body)
  (#match? @_body "\n")
  (#set! tag "deleted-bodies:docstring")
  (#set! tag "test-bodies:docstring"))
