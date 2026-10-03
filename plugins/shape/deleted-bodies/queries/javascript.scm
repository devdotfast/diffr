; inherits: builtin:shared/queries/javascript.scm, builtin:shared/queries/javascript-docstrings.scm
((function_declaration body: (statement_block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "deleted-bodies:function"))
((generator_function_declaration body: (statement_block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "deleted-bodies:function"))
((method_definition body: (statement_block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "deleted-bodies:function"))
((variable_declarator
   name: (identifier)
   value: [
     (arrow_function body: (statement_block "{" @fold.open "}" @fold.close) @fold)
     (function_expression body: (statement_block "{" @fold.open "}" @fold.close) @fold)
   ])
  (#set! tag "deleted-bodies:function"))
