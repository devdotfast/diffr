; inherits: builtin:shared/queries/rust.scm
; The scopes whose first and last line stay visible above and below a change
; inside them. A scope is the whole construct, not its body: its first line is
; the line its signature starts on, however many lines that signature runs to,
; and its last is the line that closes it. The body fold the shared query
; gives the construct covers only the body, so it nests inside the scope and
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
; shared query folds them with a range of its own.
((function_item body: (block (_expression) @fold .))
  (#not-match? @fold "^(\\{|\\[|b?r?#*\")")
  (#set! tag "context:scope"))

; Function items can begin at doc comments. Keep the full declaration header.
((function_item body: (block "{" @fold.open "}" @fold.close) @fold)
  (#set! tag "context:body"))

; A binding is a scope when its value is a block of its own: an array or a
; struct, borrowed or not, a closure or a call taking one. A change inside it keeps the
; binding's first line and the line that closes it.
([
  (const_item value: [
    (array_expression) (struct_expression) (closure_expression) (macro_invocation)
    (reference_expression value: [(array_expression) (struct_expression)])
    (call_expression arguments: (arguments (closure_expression)))])
  (static_item value: [
    (array_expression) (struct_expression) (closure_expression) (macro_invocation)
    (reference_expression value: [(array_expression) (struct_expression)])
    (call_expression arguments: (arguments (closure_expression)))])
  (let_declaration value: [
    (array_expression) (struct_expression) (closure_expression) (macro_invocation)
    (reference_expression value: [(array_expression) (struct_expression)])
    (call_expression arguments: (arguments (closure_expression)))])
] @fold
  (#set! tag "context:scope"))

; Blocks with a closing line of their own: a change inside keeps it.
((if_expression) @fold
  (#set! tag "context:scope"))
((while_expression) @fold
  (#set! tag "context:scope"))

; A comment run reads as the header of the code below it, so it stays open
; when a row of unchanged code opens.
([(line_comment) (block_comment)]+ @fold
  .
  (_)
  (#set! tag "context:comment"))
