; inherits: builtin:shared/queries/rust.scm, builtin:shared/queries/rust-docstrings.scm, builtin:shared/queries/rust-tests.scm
((function_item body: (block "{" @fold.open . (_) @fold.indent "}" @fold.close) @fold)
  (#set! tag "summarize:function"))
