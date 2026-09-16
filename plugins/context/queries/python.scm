; The scopes whose first and last line stay visible above and below a change
; inside them. A scope is the whole construct, not its body: its first line is
; the line its signature starts on, however many lines that signature runs to,
; and its last is the line that closes it. The body fold the bundled rules
; give the construct covers only the body, so it nests inside the scope and
; starts a line later.
((function_definition) @fold
  (#set! tag "context:scope"))
((class_definition) @fold
  (#set! tag "context:scope"))
((for_statement) @fold
  (#set! tag "context:scope"))
((return_statement) @fold
  (#set! tag "context:scope"))
