(statement_block) @fold.body
(class_body) @fold.body
(switch_body) @fold.body
(object) @fold.collection
(array) @fold.collection
(import_statement) @fold.import
(comment) @fold.comment
(string) @fold.string
(template_string) @fold.string

[(function_declaration body: (statement_block "}" @context.close) @context.body)
 (method_definition body: (statement_block "}" @context.close) @context.body)
 (arrow_function body: (statement_block "}" @context.close) @context.body)
 (class_declaration body: (class_body "}" @context.close) @context.body)] @context.scope
[(for_statement) (switch_statement) (return_statement)] @context.boundary
(arrow_function body: (_) @context.body) @context.scope
