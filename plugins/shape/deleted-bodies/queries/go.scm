; inherits: builtin:shared/queries/go.scm, builtin:shared/queries/go-docstrings.scm
((function_declaration body: (block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "deleted-bodies:function"))
((method_declaration body: (block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "deleted-bodies:function"))
