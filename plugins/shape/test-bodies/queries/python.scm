; inherits: builtin:shared/queries/python.scm, builtin:shared/queries/python-docstrings.scm, builtin:shared/queries/python-tests.scm
((decorated_definition
   (decorator) @_mark
   definition: (function_definition name: (identifier) @_name ":" @fold.open body: (block . (_) @fold.indent) @fold))
  (#match? @_name "^test_")
  (#match? @_mark "pytest\\.mark\\.(integration|e2e)\\b")
  (#set! tag "test-bodies:integration"))
