; inherits: builtin:shared/queries/go.scm
((function_declaration body: (block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "removed-runs:function"))
((method_declaration body: (block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "removed-runs:function"))
