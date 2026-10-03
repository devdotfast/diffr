; Structure every Python view shares: blocks, collections, imports, comments
; and strings are folds whichever plugin imports this file. These patterns
; set no tags; the plugins that import them tag what they need.
;
; A block folds from the `:` that opens it, so its fold starts on the
; header line, as a Rust body folds from its `{`. A plugin that tags a block
; must capture the same `:` or its fold range conflicts with this one.
[
  (function_definition ":" @fold.open body: (block) @fold)
  (class_definition ":" @fold.open body: (block) @fold)
  (if_statement ":" @fold.open consequence: (block) @fold)
  (elif_clause ":" @fold.open consequence: (block) @fold)
  (else_clause ":" @fold.open body: (block) @fold)
  (for_statement ":" @fold.open body: (block) @fold)
  (while_statement ":" @fold.open body: (block) @fold)
  (with_statement ":" @fold.open body: (block) @fold)
  (try_statement ":" @fold.open body: (block) @fold)
  (except_clause ":" @fold.open (block) @fold)
  (finally_clause ":" @fold.open (block) @fold)
  (match_statement ":" @fold.open body: (block) @fold)
  (case_clause ":" @fold.open consequence: (block) @fold)
]
[
  (list "[" @fold.open "]" @fold.close)
  (dictionary "{" @fold.open "}" @fold.close)
  (set "{" @fold.open "}" @fold.close)
  (tuple "(" @fold.open ")" @fold.close)
] @fold
((tuple) @fold (#not-match? @fold "^\\("))
[
  (import_statement)
  (import_from_statement)
] @fold
(comment) @fold
(string) @fold
