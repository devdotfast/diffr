use super::*;
use diffr_plugin_sdk::tree::walk_mut;
use diffr_plugin_summarize::{select, Options};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

const FUNCTION: &str = "summarize:function";

/// Project with the summarizer's queries, which run only when it is on.
fn project(path: &str, before: &str, after: &str) -> (FileChange, Pairing<protocol::Source>) {
    project_with(path, before, after, DiffOptions::default())
}

fn project_with(
    path: &str,
    before: &str,
    after: &str,
    options: DiffOptions,
) -> (FileChange, Pairing<protocol::Source>) {
    let params =
        Config::from_toml("[plugins.bundled.summarize]\nenabled = true\napi_key = 'test'\n")
            .unwrap()
            .compile()
            .unwrap();
    project_compiled(path, before, after, &params, options)
}

const LARGE: &str = "def f():\n    a()\n    b()\n    c()\n\ndef g(): d()\n";

/// Answer each request with the next canned response.
fn serve(responses: Vec<(u16, String)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut bodies = Vec::new();
        for (status, body) in responses {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut request = vec![0; length];
            reader.read_exact(&mut request).unwrap();
            bodies.push(String::from_utf8(request).unwrap());
            let reason = if status == 200 { "OK" } else { "Error" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            reader.get_mut().write_all(response.as_bytes()).unwrap();
        }
        bodies
    });
    (endpoint, handle)
}

/// The label of the only function body on the after side. The scope fold
/// the context queries wrap around it is not one.
fn fold_label(sides: &tree::Pairing<tree::Source>) -> String {
    let mut labels = Vec::new();
    walk(&rhs(sides).regions, &mut |region| {
        if is_fold(region) && has_tag(region, FUNCTION) {
            labels.push(region.visibility.label.clone());
        }
    });
    assert_eq!(labels.len(), 1, "{labels:?}");
    labels.remove(0)
}

fn gemini_answer(items: &[(u32, &str)]) -> String {
    let answers: Vec<_> = items
        .iter()
        .map(|(id, text)| json!({"id": id, "pseudocode": text}))
        .collect();
    json!({"candidates": [{"content": {"parts": [{"text": serde_json::to_string(&answers).unwrap()}]}}]})
        .to_string()
}

fn summarizer(endpoint: &str, retries: u32) -> Pipeline {
    summarizer_with(json!({
        "api_key": "test-key",
        "endpoint": endpoint,
        "retries": retries,
        "request_timeout_ms": 5000,
        "min_lines": 3,
    }))
}

/// A summarizer with the bundled defaults and `overrides`.
fn summarizer_with(overrides: serde_json::Value) -> Pipeline {
    bundled("summarize", overrides)
}

#[test]
fn selection_takes_new_bodies_of_at_least_min_lines() {
    let (_, sides) = project("a.py", "", LARGE);
    let selected = select(&trees(&sides), 3);
    assert_eq!(selected.len(), 1);
    assert_eq!((selected[0].1, selected[0].2), (2, 4));
    let (_, sides) = project("a.py", LARGE, LARGE);
    assert!(select(&trees(&sides), 3).is_empty());
}

#[test]
fn selection_reads_newness_from_the_lines_when_the_match_fell_back() {
    let before = "def keep():\n    a = 1\n    b = 2\n    return a + b\n";
    let after = "def keep():\n    a = 1\n    b = 2\n    return a + b\n\ndef fresh():\n    x = 1\n    y = 2\n    return x + y\n";
    let (_, sides) = project_with(
        "a.py",
        before,
        after,
        DiffOptions {
            graph_limit: 1,
            ..DiffOptions::default()
        },
    );
    // Nothing matched, so no fold is paired; the lines still are. Only the
    // added body, whose lines pair with nothing, is new.
    let selected = select(&trees(&sides), 3);
    assert_eq!(selected.len(), 1, "{selected:?}");
    assert_eq!((selected[0].1, selected[0].2), (7, 9));
}

#[test]
fn selection_takes_outermost_function_bodies_only() {
    // A method inside an impl: the impl's declaration_list is a body but
    // not a function, so the method is the outermost selection.
    let after = "impl A {\n    fn m(&self) {\n        a();\n        b();\n        c();\n        let f = || {\n            d();\n            e();\n            g();\n        };\n        f();\n    }\n}\n";
    let (_, sides) = project("a.rs", "", after);
    let selected = select(&trees(&sides), 3);
    assert_eq!(selected.len(), 1, "{selected:?}");
    assert_eq!((selected[0].1, selected[0].2), (3, 11));
    // Below the threshold, nothing.
    assert!(select(&trees(&sides), 30).is_empty());
}

#[test]
fn selection_skips_test_bodies_and_collapsed_folds() {
    let after = "#[test]\nfn t() {\n    a();\n    b();\n    c();\n}\n\nfn f() {\n    a();\n    b();\n    c();\n}\n";
    let (file, mut sides) = project("a.rs", "", after);
    let selected = select(&trees(&sides), 3);
    assert_eq!(selected.len(), 1, "{selected:?}");
    assert_eq!((selected[0].1, selected[0].2), (9, 11));
    run("test-bodies", json!({"min_lines": 3}), &file, &mut sides);
    let mut sides = trees(&sides);
    let (tree::Pairing::Both { rhs, .. } | tree::Pairing::RightOnly { rhs }) = &mut sides else {
        panic!("an after side");
    };
    walk_mut(&mut rhs.regions, &mut |region| {
        if region.range.start.line == 8 {
            region.visibility.collapsed = true;
        }
    });
    assert!(select(&sides, 3).is_empty());
}

#[test]
fn long_summaries_are_discarded_and_the_body_stays_open() {
    let (file, mut sides) = project("a.py", "", LARGE);
    let id = select(&trees(&sides), 3)[0].0;
    let (endpoint, server) = serve(vec![(200, gemini_answer(&[(id, "a()\nb()\nc()")]))]);
    summarizer(&endpoint, 0).run(&file, &mut sides).unwrap();
    let sides = trees(&sides);
    server.join().unwrap();
    let mut folds = Vec::new();
    walk(&rhs(&sides).regions, &mut |region| {
        if is_fold(region) && has_tag(region, FUNCTION) {
            folds.push((region.visibility.collapsed, region.visibility.label.clone()));
        }
    });
    // No plugin collapsed it, so it has no label.
    assert_eq!(folds, vec![(false, String::new())]);
}

#[test]
fn summaries_collapse_selected_folds_behind_pseudocode() {
    let (file, mut sides) = project("a.py", "", LARGE);
    let id = select(&trees(&sides), 3)[0].0;
    let (endpoint, server) = serve(vec![(200, gemini_answer(&[(id, "call a, b, c")]))]);
    summarizer(&endpoint, 0).run(&file, &mut sides).unwrap();
    let sides = trees(&sides);
    let bodies = server.join().unwrap();
    assert!(bodies[0].contains("thinkingBudget"));
    assert!(bodies[0].contains(&format!("fold {id}: lines 2-4")));
    // `g` is a one-line function, whose body is not a region.
    assert_eq!(fold_label(&sides), "call a, b, c");
    let mut collapsed = Vec::new();
    walk(&rhs(&sides).regions, &mut |region| {
        if is_fold(region) && has_tag(region, FUNCTION) {
            collapsed.push(region.visibility.collapsed);
        }
    });
    assert_eq!(collapsed, [true]);
}

#[test]
fn a_docstring_is_sent_and_only_a_verbatim_sentence_from_it_is_kept() {
    let after = "/// Sums three numbers.\n/// Used by tests.\nfn total(a: u32, b: u32, c: u32) -> u32 {\n    let x = a;\n    let y = b;\n    let z = c;\n    x + y + z\n}\n";
    let answer = |id: u32, summary: &str| {
        let answers = vec![json!({"id": id, "summary": summary, "pseudocode": "return a + b + c"})];
        json!({"candidates": [{"content": {"parts": [{"text": serde_json::to_string(&answers).unwrap()}]}}]})
            .to_string()
    };
    let body_label = |sides: &tree::Pairing<tree::Source>| {
        let mut labels = Vec::new();
        walk(&rhs(sides).regions, &mut |region| {
            if is_fold(region) && has_tag(region, FUNCTION) {
                labels.push(region.visibility.label.clone());
            }
        });
        assert_eq!(labels.len(), 1, "{labels:?}");
        labels.remove(0)
    };
    let (file, mut sides) = project("a.rs", "", after);
    let id = select(&trees(&sides), 3)[0].0;
    let (endpoint, server) = serve(vec![(200, answer(id, "Sums three numbers."))]);
    summarizer(&endpoint, 0).run(&file, &mut sides).unwrap();
    let sides = trees(&sides);
    let bodies = server.join().unwrap();
    assert!(
        bodies[0].contains("doc: Sums three numbers. Used by tests."),
        "{}",
        bodies[0]
    );
    assert_eq!(body_label(&sides), "Sums three numbers.\nreturn a + b + c");
    assert_linked(&sides, id);

    // A sentence the docstring does not contain is dropped.
    let (file, mut sides) = project("a.rs", "", after);
    let (endpoint, server) = serve(vec![(200, answer(id, "Adds things up."))]);
    summarizer(&endpoint, 0).run(&file, &mut sides).unwrap();
    let sides = trees(&sides);
    server.join().unwrap();
    assert_eq!(body_label(&sides), "return a + b + c");
}

/// The after side's docstring shares the summarized body's fold state
/// and starts collapsed with an empty label.
fn assert_linked(sides: &tree::Pairing<tree::Source>, body: u32) {
    let rhs = rhs(sides);
    let mut state = None;
    walk(&rhs.regions, &mut |region| {
        if region.id == body {
            state = Some(region.fold_state_id);
        }
    });
    let mut docstrings = Vec::new();
    walk(&rhs.regions, &mut |region| {
        if has_tag(region, "summarize:docstring") {
            docstrings.push((
                region.fold_state_id,
                region.visibility.collapsed,
                region.visibility.label.clone(),
            ));
        }
    });
    assert_eq!(docstrings, [(state.unwrap(), true, String::new())]);
}

#[test]
fn newness_is_the_lines_inside_the_body() {
    // The docstring is unchanged and the signature line still pairs. A fold
    // covers its body alone, so that line sits outside it: what decides is
    // whether a line of the body itself pairs.
    let head = "fn keep() {}\n\n/// Sums three numbers.\n/// Used by tests.\nfn total(a: u32, b: u32, c: u32) -> u32 ";
    let one_liner = format!("{head}{{ a }}\n");
    let grown = format!("{head}{{\n    let x = a;\n    x\n}}\n");
    let after =
        format!("{head}{{\n    let x = a;\n    let y = b;\n    let z = c;\n    x + y + z\n}}\n");
    let (_, sides) = project("a.rs", &one_liner, &after);
    let states = |source: &tree::Source| {
        let mut states = Vec::new();
        walk(&source.regions, &mut |region| {
            if is_fold(region) && region.range.start.line == 2 {
                states.push(region.fold_state_id);
            }
        });
        states
    };
    let projected = trees(&sides);
    let tree::Pairing::Both { lhs, rhs: replaced } = &projected else {
        panic!("both sides");
    };
    assert_eq!(
        states(lhs),
        states(replaced),
        "the docstring is matched across sides"
    );
    // The one-liner it replaced had no body fold to match, and no line of
    // the new body pairs: the body is new.
    let mut body = None;
    walk(&replaced.regions, &mut |region| {
        if is_fold(region) && has_tag(region, FUNCTION) {
            body = Some(region.fold_state_id);
        }
    });
    let mut lhs_states = Vec::new();
    walk(&lhs.regions, &mut |region| {
        lhs_states.push(region.fold_state_id)
    });
    assert!(!lhs_states.contains(&body.expect("a function body on the after side")));
    assert_eq!(select(&projected, 3).len(), 1);
    // The same body grown from one that already had lines: `let x = a;`
    // still pairs, so this is a rewrite rather than a new body.
    let (_, sides) = project("a.rs", &grown, &after);
    assert!(select(&trees(&sides), 3).is_empty());
}

#[test]
fn the_system_prompt_is_the_configured_one() {
    let (file, mut sides) = project("a.py", "", LARGE);
    let id = select(&trees(&sides), 3)[0].0;
    let request = |overrides: serde_json::Value, sides: &mut Pairing<protocol::Source>| {
        let (endpoint, server) = serve(vec![(200, gemini_answer(&[(id, "call a, b, c")]))]);
        let mut overrides = overrides;
        overrides["api_key"] = json!("test-key");
        overrides["endpoint"] = json!(endpoint);
        overrides["min_lines"] = json!(3);
        summarizer_with(overrides).run(&file, sides).unwrap();
        let bodies = server.join().unwrap();
        let body: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
        (
            body["systemInstruction"]["parts"][0]["text"].clone(),
            body["contents"][0]["parts"][0]["text"].clone(),
        )
    };
    let (system, user) = request(json!({"system_prompt": "Answer in haiku."}), &mut sides);
    assert_eq!(system, "Answer in haiku.");
    let user = user.as_str().unwrap();
    assert!(user.starts_with("File a.py:\n"), "{user}");
    assert!(user.contains(&format!("fold {id}: lines 2-4")), "{user}");

    let (_, mut sides) = project("a.py", "", LARGE);
    let (system, _) = request(json!({}), &mut sides);
    let default: Options = serde_json::from_value(serde_json::Value::Object(
        builtin::manifest("summarize").unwrap().defaults(),
    ))
    .unwrap();
    assert_eq!(system, default.system_prompt.as_str());
    assert!(default.system_prompt.starts_with(
        "For each listed fold, rewrite that function body as short pseudocode. Keep the names."
    ));
    assert!(!default.system_prompt.contains('\n'));
}

#[test]
fn transient_failures_are_retried_then_succeed() {
    let (file, mut sides) = project("a.py", "", LARGE);
    let id = select(&trees(&sides), 3)[0].0;
    let (endpoint, server) = serve(vec![
        (503, "{}".to_owned()),
        (429, "{}".to_owned()),
        (200, gemini_answer(&[(id, "retry ok")])),
    ]);
    summarizer(&endpoint, 3).run(&file, &mut sides).unwrap();
    let sides = trees(&sides);
    assert_eq!(server.join().unwrap().len(), 3);
    let label = fold_label(&sides);
    assert!(label.ends_with("retry ok"), "{label}");
}

#[test]
fn hard_failures_and_exhausted_retries_are_run_failures() {
    let (file, sides) = project("a.py", "", LARGE);
    let (endpoint, server) = serve(vec![(400, "{\"error\": \"bad key\"}".to_owned())]);
    let error = summarizer(&endpoint, 3)
        .run(&file, &mut sides.clone())
        .unwrap_err();
    server.join().unwrap();
    assert!(error.downcast_ref::<MutationFailed>().is_some());
    assert!(format!("{error:#}").starts_with("mutation summarize: summarizer: "));
    assert!(format!("{error:#}").contains("HTTP 400"), "{error:#}");
    let (endpoint, server) = serve(vec![(500, "{}".to_owned()), (500, "{}".to_owned())]);
    let error = summarizer(&endpoint, 1)
        .run(&file, &mut sides.clone())
        .unwrap_err();
    server.join().unwrap();
    assert!(
        format!("{error:#}").contains("after 2 attempts"),
        "{error:#}"
    );
}

#[test]
fn small_files_never_call_the_model() {
    let (file, sides) = project("a.py", "", "def h():\n    e()\n");
    let pipeline = summarizer_with(json!({
        "api_key": "k",
        "endpoint": "http://127.0.0.1:1",
        "min_lines": 3,
    }));
    let moves = moves(&pipeline, &file, &sides).unwrap();
    assert!(moves.is_empty());
}

/// Exercise the same component a user loads from an external plugin folder.
#[cfg(feature = "wasm-plugin-tests")]
#[test]
fn external_component_summarizes_over_http() {
    let (file, mut sides) = project("a.py", "", LARGE);
    let id = select(&trees(&sides), 3)[0].0;
    let (endpoint, server) = serve(vec![
        (429, "{}".into()),
        (200, gemini_answer(&[(id, "call a, b, c")])),
    ]);
    let engine = super::super::wasm::engine().unwrap();
    let plugin = super::super::wasm::WasmPlugin::load(
        &engine,
        &super::super::config::ComponentSource::File(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/summarize/plugin.wasm"),
        ),
    )
    .unwrap();
    let mut options = builtin::manifest("summarize").unwrap().defaults();
    options.extend(
        json!({"api_key": "test-key", "endpoint": endpoint, "min_lines": 3, "retries": 1})
            .as_object()
            .unwrap()
            .clone(),
    );
    let mut pipeline = Pipeline::default();
    pipeline
        .push(
            "summarize",
            serde_json::Value::Object(options),
            &|host, options| plugin.create(host, options),
        )
        .unwrap();
    pipeline.run(&file, &mut sides).unwrap();
    assert_eq!(fold_label(&trees(&sides)), "call a, b, c");
    assert_eq!(server.join().unwrap().len(), 2);
}

#[test]
fn tests_are_selected_when_added_modified_unchanged_or_already_collapsed() {
    use diffr_plugin_summarize::select_with_tests;
    for (path, before, after) in [
        (
            "a.py",
            "def test_it():\n    setup()\n    act()\n    check()\n",
            "def test_it():\n    setup()\n    act()\n    check_new()\n",
        ),
        (
            "a.rs",
            "#[test]\nfn it_works() {\n    setup();\n    act();\n    check();\n}\n",
            "#[test]\nfn it_works() {\n    setup();\n    act();\n    check_new();\n}\n",
        ),
        (
            "a.go",
            "package a\nfunc TestIt(t *testing.T) {\n    setup()\n    act()\n    check()\n}\n",
            "package a\nfunc TestIt(t *testing.T) {\n    setup()\n    act()\n    checkNew()\n}\n",
        ),
        (
            "a.ts",
            "test('it', () => {\n    setup();\n    act();\n    check();\n});\n",
            "test('it', () => {\n    setup();\n    act();\n    checkNew();\n});\n",
        ),
    ] {
        for old in ["", before, after] {
            // Entirely identical files bypass parsing. Keep a change outside
            // the test to exercise an unchanged body in a diffed file.
            let comment = if path.ends_with(".py") { "#" } else { "//" };
            let after = format!("{after}\n{comment} changed elsewhere\n");
            let (file, mut sides) = project(path, old, &after);
            assert_eq!(
                select_with_tests(&trees(&sides), 3, Some(3)).len(),
                1,
                "{path}: {old}"
            );
            assert!(select_with_tests(&trees(&sides), 3, None).is_empty());
            assert!(select_with_tests(&trees(&sides), 3, Some(30)).is_empty());
            run("test-bodies", json!({"min_lines": 3}), &file, &mut sides);
            assert_eq!(select_with_tests(&trees(&sides), 3, Some(3)).len(), 1);
        }
    }
}

#[test]
fn suites_select_individual_tests_and_preserve_nested_summary_folds() {
    use diffr_plugin_summarize::select_with_tests;
    for (path, after, outer_tag) in [
        ("a.rs", "#[cfg(test)]\nmod tests {\n    #[test]\n    fn one() {\n        setup();\n        act();\n        check();\n    }\n    #[test]\n    fn two() {\n        setup();\n        act();\n        check();\n    }\n}\n", "test-bodies:module"),
        ("a.ts", "describe('suite', () => {\n    it('one', () => {\n        setup();\n        act();\n        check();\n    });\n    test('two', () => {\n        setup();\n        act();\n        check();\n    });\n});\n", "test-bodies:test"),
    ] {
        let (file, mut sides) = project(path, "", after);
        let selected = select_with_tests(&trees(&sides), 3, Some(3));
        assert_eq!(selected.len(), 2, "{path}: {selected:?}");
        let (endpoint, server) = serve(vec![(200, gemini_answer(&[(selected[0].0, "setup; act; check one"), (selected[1].0, "setup; act; check two")]))]);
        summarizer_with(json!({"api_key": "test", "endpoint": endpoint, "test_min_lines": 3})).run(&file, &mut sides).unwrap();
        run("test-bodies", json!({"min_lines": 3}), &file, &mut sides);
        server.join().unwrap();
        run("group", json!({}), &file, &mut sides);
        let sides = trees(&sides);
        let mut found = 0;
        let mut outer_state = None;
        walk(&rhs(&sides).regions, &mut |region| {
            if has_tag(region, outer_tag) && !selected.iter().any(|s| s.0 == region.id) {
                assert!(region.visibility.collapsed);
                outer_state = Some(region.fold_state_id);
            }
            if selected.iter().any(|s| s.0 == region.id) {
                assert!(region.visibility.collapsed);
                assert!(region.visibility.label.starts_with("setup; act; check"));
                assert_ne!(Some(region.fold_state_id), outer_state);
                found += 1;
            }
        });
        assert!(outer_state.is_some());
        assert_eq!(found, 2);
    }
}
