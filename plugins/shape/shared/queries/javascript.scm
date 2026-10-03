; Structure every JavaScript and TypeScript view shares: blocks, collections,
; imports and strings are folds whichever plugin imports this file.
; Comments are left to javascript-docstrings.scm, which folds a run of them as
; one region. These patterns set no tags; the plugins that import them tag
; what they need.
[
  (statement_block "{" @fold.open "}" @fold.close)
  (class_body "{" @fold.open "}" @fold.close)
  (switch_body "{" @fold.open "}" @fold.close)
] @fold
[
  (object "{" @fold.open "}" @fold.close)
  (array "[" @fold.open "]" @fold.close)
] @fold
(import_statement) @fold
[
  (string)
  (template_string)
] @fold
