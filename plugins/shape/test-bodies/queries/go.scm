; inherits: builtin:shared/queries/go.scm, builtin:shared/queries/go-docstrings.scm
((function_declaration name: (identifier) @_name body: (block "{" @fold.open "}" @fold.close) @fold)
  (#match? @_name "^Test")
  (#set! tag "test-bodies:test"))
