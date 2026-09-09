(block) @fold.body
(literal_value) @fold.collection
(field_declaration_list) @fold.body
(import_declaration) @fold.import
(comment) @fold.comment
(raw_string_literal) @fold.string
(interpreted_string_literal) @fold.string
(function_declaration name: (identifier) @name body: (block) @fold.test
  (#match? @name "^Test"))

[(function_declaration body: (block "}" @context.close) @context.body)
 (method_declaration body: (block "}" @context.close) @context.body)] @context.scope
[(for_statement) (expression_switch_statement) (return_statement)] @context.boundary
