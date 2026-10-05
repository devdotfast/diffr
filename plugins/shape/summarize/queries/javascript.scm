; inherits: builtin:shared/queries/javascript.scm, builtin:shared/queries/javascript-docstrings.scm, builtin:shared/queries/javascript-tests.scm
((function_declaration body: (statement_block "{" @fold.open . (_) @fold.indent "}" @fold.close) @fold)
  (#set! tag "summarize:function"))
((generator_function_declaration body: (statement_block "{" @fold.open . (_) @fold.indent "}" @fold.close) @fold)
  (#set! tag "summarize:function"))
((method_definition body: (statement_block "{" @fold.open . (_) @fold.indent "}" @fold.close) @fold)
  (#set! tag "summarize:function"))
((variable_declarator
   name: (identifier)
   value: [
     (arrow_function body: (statement_block "{" @fold.open . (_) @fold.indent "}" @fold.close) @fold)
     (function_expression body: (statement_block "{" @fold.open . (_) @fold.indent "}" @fold.close) @fold)
   ])
  (#set! tag "summarize:function"))
