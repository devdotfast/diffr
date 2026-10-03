; Structure every Rust view shares: bodies, collections, imports, block
; comments and strings are folds whichever plugin imports this file. These
; patterns set no tags; the plugins that import them tag what they need.
[
  (block "{" @fold.open "}" @fold.close)
  (field_declaration_list "{" @fold.open "}" @fold.close)
  (declaration_list "{" @fold.open "}" @fold.close)
  (match_block "{" @fold.open "}" @fold.close)
] @fold
[
  (array_expression "[" @fold.open "]" @fold.close)
  (field_initializer_list "{" @fold.open "}" @fold.close)
] @fold
(use_declaration) @fold
(block_comment) @fold
[
  (string_literal)
  (raw_string_literal)
] @fold
