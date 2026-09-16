; The scopes whose first and last line stay visible above and below a change
; inside them. A scope is the whole construct, not its body: its first line is
; the line its signature starts on, however many lines that signature runs to,
; and its last is the line that closes it. The body fold the bundled rules
; give the construct covers only the body, so it nests inside the scope and
; starts a line later.
((function_item) @fold
  (#set! tag "context:scope"))
((impl_item) @fold
  (#set! tag "context:scope"))
((mod_item) @fold
  (#set! tag "context:scope"))
((struct_item) @fold
  (#set! tag "context:scope"))
((for_expression) @fold
  (#set! tag "context:scope"))
((loop_expression) @fold
  (#set! tag "context:scope"))
((match_expression) @fold
  (#set! tag "context:scope"))
((return_expression) @fold
  (#set! tag "context:scope"))
; A function's tail expression. Blocks, arrays and strings are left out: the
; bundled rules fold them with a range of its own.
((function_item body: (block (_expression) @fold .))
  (#not-match? @fold "^(\\{|\\[|b?r?#*\")")
  (#set! tag "context:scope"))
