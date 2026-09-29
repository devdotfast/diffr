module Scanner

(* outer comment (* nested comment *) ends here *)
let message = """first
second"""

let compute values =
    values
    |> List.map (fun value ->
        match value with
        | Some x ->
            let doubled = x * 2
            doubled + 1
        | None -> 0)
