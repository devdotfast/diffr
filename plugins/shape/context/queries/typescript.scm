; inherits: javascript.scm, builtin:shared/queries/typescript.scm
; Type declarations are scopes too: a change inside keeps their first and
; last line. An exported one is matched on its export, so the scope starts
; on the line with `export`, not the line after it.
(program
  [
    (interface_declaration)
    (type_alias_declaration value: (object_type))
    (enum_declaration)
  ] @fold
  (#set! tag "context:scope"))
(statement_block
  [
    (interface_declaration)
    (type_alias_declaration value: (object_type))
    (enum_declaration)
  ] @fold
  (#set! tag "context:scope"))
((export_statement declaration: [
  (interface_declaration)
  (type_alias_declaration value: (object_type))
  (enum_declaration)
]) @fold (#set! tag "context:scope"))
