; inherits: builtin:shared/queries/javascript.scm
; The scopes whose first and last line stay visible above and below a change
; inside them. A scope is the whole construct, not its body: its first line is
; the line its signature starts on, however many lines that signature runs to,
; and its last is the line that closes it. The body fold the shared query
; gives the construct covers only the body, so it nests inside the scope and
; starts a line later.
((function_declaration) @fold
  (#set! tag "context:scope"))
((generator_function_declaration) @fold
  (#set! tag "context:scope"))
((method_definition) @fold
  (#set! tag "context:scope"))
; An arrow function is a scope whether its body is a block or an expression.
((arrow_function) @fold
  (#set! tag "context:scope"))
((class_declaration) @fold
  (#set! tag "context:scope"))
((for_statement) @fold
  (#set! tag "context:scope"))
((switch_statement) @fold
  (#set! tag "context:scope"))
((return_statement) @fold
  (#set! tag "context:scope"))
