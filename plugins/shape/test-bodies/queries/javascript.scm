; inherits: builtin:shared/queries/javascript.scm, builtin:shared/queries/javascript-docstrings.scm
((call_expression
   function: (identifier) @_name
   arguments: (arguments [
     (arrow_function body: (statement_block "{" @fold.open . (_) @fold.indent "}" @fold.close) @fold)
     (function_expression body: (statement_block "{" @fold.open . (_) @fold.indent "}" @fold.close) @fold)
   ]))
  (#match? @_name "^(it|test|describe)$")
  (#set! tag "test-bodies:test"))
