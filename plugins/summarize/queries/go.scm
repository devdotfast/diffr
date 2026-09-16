; inherits: builtin:shared/queries/go.scm, builtin:shared/queries/go-docstrings.scm
((function_declaration body: (block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "summarize:function"))
((method_declaration body: (block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "summarize:function"))
((function_declaration name: (identifier) @_name body: (block "{" @fold.open "}" @fold.close) @fold)
  (#match? @_name "^Test")
  (#set! tag "summarize:test"))
