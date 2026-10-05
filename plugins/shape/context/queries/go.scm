; inherits: builtin:shared/queries/go.scm
; The scopes whose first and last line stay visible above and below a change
; inside them. A scope is the whole construct, not its body: its first line is
; the line its signature starts on, however many lines that signature runs to,
; and its last is the line that closes it. The body fold the shared query
; gives the construct covers only the body, so it nests inside the scope and
; starts a line later.
((function_declaration) @fold
  (#set! tag "context:scope"))
((method_declaration) @fold
  (#set! tag "context:scope"))
((for_statement) @fold
  (#set! tag "context:scope"))
((expression_switch_statement) @fold
  (#set! tag "context:scope"))
((return_statement) @fold
  (#set! tag "context:scope"))

; Blocks with a closing line of their own: a change inside keeps it.
((if_statement) @fold
  (#set! tag "context:scope"))
((type_switch_statement) @fold
  (#set! tag "context:scope"))
((select_statement) @fold
  (#set! tag "context:scope"))

; A comment run reads as the header of the code below it, so it stays open
; when a row of unchanged code opens.
((comment)+ @fold
  .
  (_)
  (#set! tag "context:comment"))
