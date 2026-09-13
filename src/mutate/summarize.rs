//! The built-in summarizer: large new function bodies become Python-style
//! pseudocode, shown in place of the collapsed body.
use super::docstrings;
#[cfg(test)]
use super::walk;
use super::{collapse, ids, is_fold, line_count, summary_label, walk_mut, Failure, FoldMutation};
use crate::config::{Provider, SummarizeConfig};
use crate::hash::DftHashSet;
use crate::pairing::Pairing;
use crate::protocol::{FileChange, Source};
use crate::protocol::{Node, Region};
use anyhow::{anyhow, Context as _};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::sync::Semaphore;

const SYSTEM: &str = "For each listed fold, rewrite that function body as short python-flavored \
pseudocode. Keep the names. No prose, no comments, no code fences. Use as few lines as \
possible: about one pseudocode line per five source lines, and never more than a third \
of the body's lines. When a fold lists a doc, also set \"summary\" to one sentence copied \
verbatim from that doc; otherwise leave it empty. Answer with a JSON array of \
{\"id\", \"summary\", \"pseudocode\"} objects, one per fold.";

const DEFAULT_ENDPOINT: &str = "https://generativelanguage.googleapis.com";

pub(crate) struct Summarizer {
    config: SummarizeConfig,
    min_lines: usize,
    api_key: String,
    endpoint: String,
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
    limit: Semaphore,
}

/// One fold to summarize: its region id, 1-based inclusive line range, and
/// the text of its docstring when it has one.
struct Request {
    id: u32,
    first_line: u32,
    last_line: u32,
    doc: Option<String>,
}

#[derive(Deserialize)]
struct Answer {
    id: u32,
    #[serde(default)]
    summary: String,
    pseudocode: String,
}

/// What the model returned for one fold: an optional sentence quoted from
/// its docstring, and the pseudocode.
struct Summary {
    quote: Option<String>,
    pseudocode: String,
}

impl Summarizer {
    /// `None` when no API key resolves: the summarizer is then simply off.
    pub(crate) fn new(config: &SummarizeConfig) -> anyhow::Result<Option<Self>> {
        let Some(api_key) = resolve_key(config) else {
            return Ok(None);
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("diffr-summarizer")
            .enable_all()
            .build()?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()?;
        Ok(Some(Self {
            config: config.clone(),
            min_lines: config.min_lines,
            api_key,
            endpoint: config
                .endpoint
                .clone()
                .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned()),
            runtime,
            client,
            limit: Semaphore::new(config.max_concurrency),
        }))
    }

    fn url(&self) -> String {
        match self.config.provider {
            Provider::Gemini => format!(
                "{}/v1beta/models/{}:generateContent",
                self.endpoint.trim_end_matches('/'),
                self.config.model
            ),
        }
    }

    fn prompt(&self, path: &str, language: Option<&str>, src: &str, folds: &[Request]) -> String {
        let numbered: Vec<String> = src
            .split_terminator('\n')
            .enumerate()
            .map(|(index, line)| format!("{:5} | {line}", index + 1))
            .collect();
        let ranges: Vec<String> = folds
            .iter()
            .map(|fold| {
                let doc = fold
                    .doc
                    .as_ref()
                    .map(|doc| format!("\n  doc: {doc}"))
                    .unwrap_or_default();
                format!(
                    "- fold {}: lines {}-{}{doc}",
                    fold.id, fold.first_line, fold.last_line
                )
            })
            .collect();
        format!(
            "File {path} ({}):\n\n{}\n\nFolds:\n{}",
            language.unwrap_or("unknown language"),
            numbered.join("\n"),
            ranges.join("\n")
        )
    }

    /// One request per file, retried on transient failures. Any other
    /// failure is a run-level failure.
    fn complete(
        &self,
        path: &str,
        language: Option<&str>,
        src: &str,
        folds: &[Request],
    ) -> anyhow::Result<BTreeMap<u32, Summary>> {
        let body = json!({
            "systemInstruction": {"parts": [{"text": SYSTEM}]},
            "contents": [{"role": "user", "parts": [{"text": self.prompt(path, language, src, folds)}]}],
            "generationConfig": {
                "temperature": 0,
                "maxOutputTokens": 600 * folds.len() + 200,
                "thinkingConfig": {"thinkingBudget": 0},
                "responseMimeType": "application/json",
                "responseSchema": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": {
                            "id": {"type": "INTEGER"},
                            "summary": {"type": "STRING"},
                            "pseudocode": {"type": "STRING"},
                        },
                        "required": ["id", "pseudocode"],
                    },
                },
            },
        });
        let url = self.url();
        let failed = |message: String| anyhow!("{}: {message}", self.config.model);
        let text = self.runtime.block_on(async {
            let _permit = self
                .limit
                .acquire()
                .await
                .expect("semaphore is never closed");
            let mut attempt = 0;
            loop {
                let response = self
                    .client
                    .post(&url)
                    .header("x-goog-api-key", &self.api_key)
                    .json(&body)
                    .send()
                    .await;
                let retry = match response {
                    Ok(response) if response.status().is_success() => {
                        return response
                            .json::<serde_json::Value>()
                            .await
                            .map_err(|error| failed(error.to_string()));
                    }
                    Ok(response)
                        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
                            || response.status().is_server_error() =>
                    {
                        format!("HTTP {}", response.status())
                    }
                    Ok(response) => {
                        let status = response.status();
                        let detail = response.text().await.unwrap_or_default();
                        return Err(failed(format!(
                            "HTTP {status} {}",
                            detail.chars().take(200).collect::<String>()
                        )));
                    }
                    Err(error)
                        if error.is_timeout() || error.is_connect() || error.is_request() =>
                    {
                        error.to_string()
                    }
                    Err(error) => return Err(failed(error.to_string())),
                };
                if attempt >= self.config.retries {
                    return Err(failed(format!("{retry} after {} attempts", attempt + 1)));
                }
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(250 * (1 << attempt.min(6)))).await;
            }
        })?;
        let content = text["candidates"][0]["content"]["parts"]
            .as_array()
            .and_then(|parts| parts.last())
            .and_then(|part| part["text"].as_str())
            .ok_or_else(|| failed("no text in the response".to_owned()))?;
        let answers: Vec<Answer> =
            serde_json::from_str(content).map_err(|error| failed(format!("{error}: {content}")))?;
        let mut texts = BTreeMap::new();
        for answer in answers {
            if !folds.iter().any(|fold| fold.id == answer.id) {
                return Err(failed(format!("answered for unknown fold {}", answer.id)));
            }
            if !answer.pseudocode.trim().is_empty() {
                let doc = folds
                    .iter()
                    .find(|fold| fold.id == answer.id)
                    .and_then(|fold| fold.doc.as_deref());
                texts.insert(
                    answer.id,
                    Summary {
                        quote: quoted(doc, &answer.summary),
                        pseudocode: answer.pseudocode.trim().to_owned(),
                    },
                );
            }
        }
        Ok(texts)
    }
}

/// New function bodies on the after side of at least `min_lines` lines:
/// folds tagged `function` with no counterpart. Only the outermost
/// qualifying body is taken, never one nested inside it; test bodies and
/// folds that already start collapsed are skipped.
pub(crate) fn select(sides: &Pairing<Source>, min_lines: usize) -> Vec<(u32, u32, u32)> {
    let (lhs_ids, rhs) = match sides {
        Pairing::Both { lhs, rhs } => (ids(&lhs.regions), rhs),
        Pairing::RightOnly { rhs } => (DftHashSet::default(), rhs),
        Pairing::LeftOnly { .. } => return Vec::new(),
    };
    let mut selected = Vec::new();
    fn visit(
        regions: &[Region],
        lhs_ids: &DftHashSet<u32>,
        min_lines: usize,
        selected: &mut Vec<(u32, u32, u32)>,
    ) {
        for region in regions {
            let tag = |name: &str| region.tags.iter().any(|tag| tag == name);
            if is_fold(region)
                && tag("function")
                && !tag("test")
                && !region.visibility.collapsed
                && !lhs_ids.contains(&region.alignment_id)
                && line_count(region) >= min_lines
            {
                let lines = region.range.lines();
                selected.push((region.alignment_id, lines.start + 1, lines.end));
                continue;
            }
            if let Node::Fold { children } = &region.node {
                visit(children, lhs_ids, min_lines, selected);
            }
        }
    }
    visit(&rhs.regions, &lhs_ids, min_lines, &mut selected);
    selected
}

/// The model's sentence, kept only when it really is a verbatim quote from
/// the docstring: compared with runs of whitespace collapsed.
fn quoted(doc: Option<&str>, summary: &str) -> Option<String> {
    let squash = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let (doc, sentence) = (squash(doc?), squash(summary));
    (!sentence.is_empty() && doc.contains(&sentence)).then_some(sentence)
}

/// Pseudocode earns its place only when it is clearly shorter than the
/// code: a summary with more than half the body's non-blank lines is
/// dropped and the body stays open.
fn compresses(summary: &str, body: &[&str]) -> bool {
    let summary_lines = summary
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    let body_lines = body.iter().filter(|line| !line.trim().is_empty()).count();
    summary_lines * 2 <= body_lines
}

/// The fold state of the region with this alignment id on one side.
fn fold_state_of(source: &Source, alignment_id: u32) -> u32 {
    let mut state = alignment_id;
    super::walk(&source.regions, &mut |region| {
        if region.alignment_id == alignment_id {
            state = region.fold_state_id;
        }
    });
    state
}

fn resolve_key(config: &SummarizeConfig) -> Option<String> {
    config
        .api_key
        .clone()
        .filter(|key| !key.is_empty())
        .or_else(|| {
            std::env::var("GEMINI_API_KEY")
                .ok()
                .filter(|key| !key.is_empty())
        })
        .or_else(|| {
            std::env::var("GOOGLE_API_KEY")
                .ok()
                .filter(|key| !key.is_empty())
        })
}

impl FoldMutation for Summarizer {
    fn apply(&self, file: &FileChange, sides: &mut Pairing<Source>) -> anyhow::Result<()> {
        let selected = select(sides, self.min_lines);
        let (Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs }) = sides else {
            return Ok(());
        };
        let folds: Vec<Request> = selected
            .into_iter()
            .map(|(id, first_line, last_line)| Request {
                id,
                first_line,
                last_line,
                doc: docstrings::text_for(rhs, fold_state_of(rhs, id)),
            })
            .collect();
        if folds.is_empty() {
            return Ok(());
        }
        let (Pairing::Both { rhs: after, .. } | Pairing::RightOnly { rhs: after }) = &file.file
        else {
            unreachable!("a file with after-side regions has an after path");
        };
        let language = file.language.as_deref();
        let lines: Vec<&str> = rhs.text.split_terminator('\n').collect();
        let mut texts = self
            .complete(&after.path, language, &rhs.text, &folds)
            .context(Failure::Summarizer)?;
        texts.retain(|id, summary| {
            let fold = folds
                .iter()
                .find(|fold| fold.id == *id)
                .expect("answered fold");
            let body = &lines[fold.first_line as usize - 1..fold.last_line as usize];
            compresses(&summary.pseudocode, body)
        });
        walk_mut(&mut rhs.regions, &mut |region| {
            if let Some(summary) = texts.get(&region.alignment_id) {
                let text = match &summary.quote {
                    Some(quote) => format!("{quote}\n{}", summary.pseudocode),
                    None => summary.pseudocode.clone(),
                };
                collapse(region, summary_label(language, &text));
            }
        });
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::Config;
    use crate::protocol::{project, Diff, FileRef, FileStatus, Visibility};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// Project a two-source comparison the way the stream does.
    pub(crate) fn project(path: &str, before: &str, after: &str) -> (FileChange, Pairing<Source>) {
        project_with(path, before, after, crate::options::DiffOptions::default())
    }

    pub(crate) fn project_with(
        path: &str,
        before: &str,
        after: &str,
        options: crate::options::DiffOptions,
    ) -> (FileChange, Pairing<Source>) {
        let params = Config::from_toml("").unwrap().compile().unwrap();
        let result = crate::summary::DiffResult::from_sources_with_options(
            path,
            before,
            after,
            &params,
            &crate::options::DisplayOptions::default(),
            &options,
        );
        let file_ref = FileRef {
            path: path.to_owned(),
            oid: String::new(),
            mode: String::new(),
        };
        let file = FileChange {
            file: Pairing::Both {
                lhs: file_ref.clone(),
                rhs: file_ref,
            },
            status: FileStatus::Modified,
            category: None,
            language: crate::parse::guess_language::guess(std::path::Path::new(path), "", &[])
                .map(|language| crate::parse::guess_language::language_name(language).to_owned()),
            visibility: Visibility::default(),
        };
        let diff = project::diff(
            &result,
            project::Inputs {
                file: &file.file,
                sizes: (before.len() as u64, after.len() as u64),
                syntax: (Vec::new(), Vec::new()),
            },
        );
        let Diff::Text { sides, .. } = diff else {
            panic!("text diff expected");
        };
        (file, sides)
    }

    const LARGE: &str = "def f():\n    a()\n    b()\n    c()\n\ndef g():\n    d()\n";

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

    /// The label of the only fold region on the after side.
    fn fold_label(sides: &Pairing<Source>) -> String {
        let mut labels = Vec::new();
        let (Pairing::Both { rhs: rhs_side, .. } | Pairing::RightOnly { rhs: rhs_side }) = &sides
        else {
            panic!("an after side");
        };
        walk(&rhs_side.regions, &mut |region| {
            if is_fold(region) {
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

    fn summarizer(endpoint: &str, retries: u32) -> Summarizer {
        let config = SummarizeConfig {
            api_key: Some("test-key".to_owned()),
            endpoint: Some(endpoint.to_owned()),
            retries,
            timeout_ms: 5000,
            min_lines: 3,
            ..SummarizeConfig::default()
        };
        Summarizer::new(&config).unwrap().unwrap()
    }

    #[test]
    fn selection_takes_new_bodies_of_at_least_min_lines() {
        let (_, sides) = project("a.py", "", LARGE);
        let selected = select(&sides, 3);
        assert_eq!(selected.len(), 1);
        assert_eq!((selected[0].1, selected[0].2), (2, 4));
        let (_, sides) = project("a.py", LARGE, LARGE);
        assert!(select(&sides, 3).is_empty());
    }

    #[test]
    fn selection_still_finds_new_bodies_when_the_match_fell_back() {
        let before = "def keep():\n    a = 1\n    b = 2\n    return a + b\n";
        let after = "def keep():\n    a = 1\n    b = 2\n    return a + b\n\ndef fresh():\n    x = 1\n    y = 2\n    return x + y\n";
        let (_, sides) = project_with(
            "a.py",
            before,
            after,
            crate::options::DiffOptions {
                graph_limit: 1,
                ..crate::options::DiffOptions::default()
            },
        );
        let selected = select(&sides, 3);
        assert_eq!(selected.len(), 1, "{selected:?}");
        assert_eq!((selected[0].1, selected[0].2), (7, 9));
    }

    #[test]
    fn selection_takes_outermost_function_bodies_only() {
        // A method inside an impl: the impl's declaration_list is a body but
        // not a function, so the method is the outermost selection.
        let after = "impl A {\n    fn m(&self) {\n        a();\n        b();\n        c();\n        let f = || {\n            d();\n            e();\n            g();\n        };\n        f();\n    }\n}\n";
        let (_, sides) = project("a.rs", "", after);
        let selected = select(&sides, 3);
        assert_eq!(selected.len(), 1, "{selected:?}");
        assert_eq!((selected[0].1, selected[0].2), (2, 12));
        // Below the threshold, nothing.
        assert!(select(&sides, 30).is_empty());
    }

    #[test]
    fn selection_skips_test_bodies_and_collapsed_folds() {
        let after = "#[test]\nfn t() {\n    a();\n    b();\n    c();\n}\n\nfn f() {\n    a();\n    b();\n    c();\n}\n";
        let (file, mut sides) = project("a.rs", "", after);
        let selected = select(&sides, 3);
        assert_eq!(selected.len(), 1, "{selected:?}");
        assert_eq!((selected[0].1, selected[0].2), (8, 11));
        crate::mutate::collapse::TestBodies
            .apply(&file, &mut sides)
            .unwrap();
        let (Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs }) = &mut sides else {
            panic!("rhs expected");
        };
        walk_mut(&mut rhs.regions, &mut |region| {
            if region.range.start.line == 7 {
                region.visibility.collapsed = true;
            }
        });
        assert!(select(&sides, 3).is_empty());
    }

    #[test]
    fn long_summaries_are_discarded_and_the_body_stays_open() {
        let (file, mut sides) = project("a.py", "", LARGE);
        let id = select(&sides, 3)[0].0;
        let (endpoint, server) = serve(vec![(200, gemini_answer(&[(id, "a()\nb()\nc()")]))]);
        summarizer(&endpoint, 0).apply(&file, &mut sides).unwrap();
        server.join().unwrap();
        let (Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs }) = &sides else {
            panic!("an after side");
        };
        let mut folds = Vec::new();
        walk(&rhs.regions, &mut |region| {
            if is_fold(region) {
                folds.push((region.visibility.collapsed, region.visibility.label.clone()));
            }
        });
        assert_eq!(folds, vec![(false, "Body".to_owned())]);
    }

    #[test]
    fn summaries_collapse_selected_folds_with_a_pseudocode_comment() {
        let (file, mut sides) = project("a.py", "", LARGE);
        let id = select(&sides, 3)[0].0;
        let (endpoint, server) = serve(vec![(200, gemini_answer(&[(id, "call a, b, c")]))]);
        summarizer(&endpoint, 0).apply(&file, &mut sides).unwrap();
        let bodies = server.join().unwrap();
        assert!(bodies[0].contains("thinkingBudget"));
        assert!(bodies[0].contains(&format!("fold {id}: lines 2-4")));
        let (Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs }) = &sides else {
            panic!("an after side");
        };
        let mut labels = Vec::new();
        walk(&rhs.regions, &mut |region| {
            if is_fold(region) {
                labels.push((region.visibility.collapsed, region.visibility.label.clone()));
            }
        });
        // `g` has a one-line body, which is not a region.
        assert_eq!(
            labels,
            vec![(
                true,
                "# pseudocode
call a, b, c"
                    .to_owned()
            )]
        );
    }

    #[test]
    fn a_docstring_is_sent_and_only_a_verbatim_sentence_from_it_is_kept() {
        let after = "/// Sums three numbers. Used by tests.\nfn total(a: u32, b: u32, c: u32) -> u32 {\n    let x = a;\n    let y = b;\n    let z = c;\n    x + y + z\n}\n";
        let (file, mut sides) = project("a.rs", "", after);
        super::docstrings::Docstrings
            .apply(&file, &mut sides)
            .unwrap();
        let id = select(&sides, 3)[0].0;
        let answer = |summary: &str| {
            let answers =
                vec![json!({"id": id, "summary": summary, "pseudocode": "return a + b + c"})];
            json!({"candidates": [{"content": {"parts": [{"text": serde_json::to_string(&answers).unwrap()}]}}]})
                .to_string()
        };
        let (endpoint, server) = serve(vec![(200, answer("Sums three numbers."))]);
        let summarizer = summarizer(&endpoint, 0);
        summarizer.apply(&file, &mut sides).unwrap();
        let bodies = server.join().unwrap();
        assert!(
            bodies[0].contains("doc: Sums three numbers. Used by tests."),
            "{}",
            bodies[0]
        );
        assert_eq!(
            fold_label(&sides),
            "// pseudocode\nSums three numbers.\nreturn a + b + c"
        );

        // A sentence the docstring does not contain is dropped.
        let (file, mut sides) = project("a.rs", "", after);
        super::docstrings::Docstrings
            .apply(&file, &mut sides)
            .unwrap();
        let (endpoint, server) = serve(vec![(200, answer("Adds things up."))]);
        super::tests::summarizer(&endpoint, 0)
            .apply(&file, &mut sides)
            .unwrap();
        server.join().unwrap();
        assert_eq!(fold_label(&sides), "// pseudocode\nreturn a + b + c");
    }

    #[test]
    fn transient_failures_are_retried_then_succeed() {
        let (file, mut sides) = project("a.py", "", LARGE);
        let id = select(&sides, 3)[0].0;
        let (endpoint, server) = serve(vec![
            (503, "{}".to_owned()),
            (429, "{}".to_owned()),
            (200, gemini_answer(&[(id, "retry ok")])),
        ]);
        summarizer(&endpoint, 3).apply(&file, &mut sides).unwrap();
        assert_eq!(server.join().unwrap().len(), 3);
        let label = fold_label(&sides);
        assert!(label.ends_with("retry ok"), "{label}");
    }

    #[test]
    fn hard_failures_and_exhausted_retries_are_run_failures() {
        let (file, mut sides) = project("a.py", "", LARGE);
        let (endpoint, server) = serve(vec![(400, "{\"error\": \"bad key\"}".to_owned())]);
        let error = summarizer(&endpoint, 3)
            .apply(&file, &mut sides)
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.downcast_ref::<Failure>(), Some(&Failure::Summarizer));
        assert!(format!("{error:#}").contains("HTTP 400"), "{error:#}");
        let (endpoint, server) = serve(vec![(500, "{}".to_owned()), (500, "{}".to_owned())]);
        let error = summarizer(&endpoint, 1)
            .apply(&file, &mut sides)
            .unwrap_err();
        server.join().unwrap();
        assert!(
            format!("{error:#}").contains("after 2 attempts"),
            "{error:#}"
        );
        let (Pairing::Both { rhs: rhs_side, .. } | Pairing::RightOnly { rhs: rhs_side }) = &sides
        else {
            panic!("an after side");
        };
        assert!(!rhs_side.regions[0].visibility.collapsed);
    }

    #[test]
    fn small_files_never_call_the_model() {
        let (file, mut sides) = project("a.py", "", "def h():\n    e()\n");
        let config = SummarizeConfig {
            api_key: Some("k".to_owned()),
            endpoint: Some("http://127.0.0.1:1".to_owned()),
            min_lines: 3,
            ..SummarizeConfig::default()
        };
        Summarizer::new(&config)
            .unwrap()
            .unwrap()
            .apply(&file, &mut sides)
            .unwrap();
    }

    #[test]
    fn no_key_means_no_summarizer() {
        let config = SummarizeConfig {
            api_key: None,
            ..SummarizeConfig::default()
        };
        // Only meaningful when the environment carries no key.
        if std::env::var_os("GEMINI_API_KEY").is_none()
            && std::env::var_os("GOOGLE_API_KEY").is_none()
        {
            assert!(Summarizer::new(&config).unwrap().is_none());
        }
    }
}
