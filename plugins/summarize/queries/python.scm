; inherits: builtin:shared/queries/python.scm, builtin:shared/queries/python-docstrings.scm
((function_definition ":" @fold.open body: (block) @fold)
  (#set! tag "summarize:function"))
((function_definition name: (identifier) @_name ":" @fold.open body: (block) @fold)
  (#match? @_name "^test_")
  (#set! tag "summarize:test"))
