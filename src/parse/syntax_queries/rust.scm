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

(function_item body: (block "}" @context.close) @context.body) @context.scope
(impl_item body: (declaration_list "}" @context.close) @context.body) @context.scope
(mod_item body: (declaration_list "}" @context.close) @context.body) @context.scope
(struct_item body: (field_declaration_list "}" @context.close) @context.body) @context.scope
[(for_expression) (loop_expression) (match_expression) (return_expression)] @context.boundary
(function_item body: (block (_expression) @context.boundary .))
