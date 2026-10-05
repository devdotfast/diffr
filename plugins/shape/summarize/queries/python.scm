; inherits: builtin:shared/queries/python.scm, builtin:shared/queries/python-docstrings.scm, builtin:shared/queries/python-tests.scm
((function_definition ":" @fold.open body: (block . (_) @fold.indent) @fold)
  (#set! tag "summarize:function"))
