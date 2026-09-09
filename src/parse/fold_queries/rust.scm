(block) @fold.body
(field_declaration_list) @fold.body
(declaration_list) @fold.body
(match_block) @fold.body
(array_expression) @fold.collection
(field_initializer_list) @fold.collection
(use_declaration) @fold.import
(line_comment) @fold.comment
(block_comment) @fold.comment
(string_literal) @fold.string
(raw_string_literal) @fold.string
((attribute_item) @attribute . (function_item body: (block) @fold.test)
  (#match? @attribute "test"))
