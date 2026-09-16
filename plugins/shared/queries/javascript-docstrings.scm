; Docstrings, imported by the plugins that collapse function bodies: each
; links a body it collapses to the docstring tagged with its own name (see
; `docstring_of` in crates/diffr-plugin-sdk/src/tree.rs). A file is included once however many
; plugins import it, so a docstring carries one tag per importer.
;
; A docstring: the run of comments directly above a function, a method, a
; declaration or an export. It is tagged only above a function whose body
; spans lines, and so is a region: the fold just before a body is then never
; the docstring of a one-line function or a declaration above it.
((comment)+ @fold
  .
  [
    (function_declaration)
    (generator_function_declaration)
    (method_definition)
    (lexical_declaration)
    (variable_declaration)
    (export_statement)
  ])
((comment)+ @fold
  .
  [
    (function_declaration body: (statement_block) @_body)
    (generator_function_declaration body: (statement_block) @_body)
    (method_definition body: (statement_block) @_body)
    (export_statement
      declaration: [
        (function_declaration body: (statement_block) @_body)
        (generator_function_declaration body: (statement_block) @_body)
      ])
    (lexical_declaration
      (variable_declarator
        value: [
          (arrow_function body: (statement_block) @_body)
          (function_expression body: (statement_block) @_body)
        ]))
    (variable_declaration
      (variable_declarator
        value: [
          (arrow_function body: (statement_block) @_body)
          (function_expression body: (statement_block) @_body)
        ]))
  ]
  (#match? @_body "\n")
  (#set! tag "deleted-bodies:docstring")
  (#set! tag "test-bodies:docstring"))
