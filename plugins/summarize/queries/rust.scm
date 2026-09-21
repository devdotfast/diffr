; inherits: builtin:shared/queries/rust.scm, builtin:shared/queries/rust-docstrings.scm
((function_item body: (block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "summarize:function"))
((function_item attributes: (attributes (attribute_item) @_attribute) body: (block "{" @fold.open "}" @fold.close) @fold)
  (#match? @_attribute "test")
  (#set! tag "summarize:test"))
