(block) @fold.body
(list) @fold.collection
(dictionary) @fold.collection
(set) @fold.collection
(tuple) @fold.collection
(import_statement) @fold.import
(import_from_statement) @fold.import
(comment) @fold.comment
(string) @fold.string
(function_definition name: (identifier) @name body: (block) @fold.test
  (#match? @name "^test_"))

[(function_definition body: (block) @context.indented_body)
 (class_definition body: (block) @context.indented_body)] @context.scope
[(for_statement) (return_statement)] @context.boundary
