; inherits: builtin:shared/queries/javascript.scm, builtin:shared/queries/javascript-docstrings.scm
((function_declaration body: (statement_block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "summarize:function"))
((generator_function_declaration body: (statement_block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "summarize:function"))
((method_definition body: (statement_block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "summarize:function"))
((variable_declarator
   name: (identifier)
   value: [
     (arrow_function body: (statement_block "{" @fold.open "}" @fold.close) @fold)
     (function_expression body: (statement_block "{" @fold.open "}" @fold.close) @fold)
   ])
  (#set! tag "summarize:function"))
((call_expression
   function: (identifier) @_name
   arguments: (arguments [
     (arrow_function body: (statement_block "{" @fold.open "}" @fold.close) @fold)
     (function_expression body: (statement_block "{" @fold.open "}" @fold.close) @fold)
   ]))
  (#match? @_name "^(it|test)$")
  (#set! tag "summarize:test"))
