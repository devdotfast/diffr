; Structure every Go view shares: blocks, literals, imports and strings are
; folds whichever plugin imports this file. Comments are left to
; go-docstrings.scm, which folds a run of them as one region. These patterns
; set no tags; the plugins that import them tag what they need.
[
  (block "{" @fold.open "}" @fold.close)
  (field_declaration_list "{" @fold.open "}" @fold.close)
] @fold
(literal_value "{" @fold.open "}" @fold.close) @fold
(import_declaration) @fold
[
  (raw_string_literal)
  (interpreted_string_literal)
] @fold
