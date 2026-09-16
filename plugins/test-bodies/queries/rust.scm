; inherits: builtin:shared/queries/rust.scm, builtin:shared/queries/rust-docstrings.scm
((function_item attributes: (attributes (attribute_item) @_attribute) body: (block "{" @fold.open "}" @fold.close) @fold)
  (#match? @_attribute "test")
  (#set! tag "test-bodies:test"))
((mod_item attributes: (attributes (attribute_item) @_attribute) body: (declaration_list "{" @fold.open "}" @fold.close) @fold)
  (#match? @_attribute "cfg\\(test\\)")
  (#set! tag "test-bodies:module"))
