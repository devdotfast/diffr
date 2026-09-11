//! The built-in summarizer: large new function bodies become Python-style
//! pseudocode, shown in place of the collapsed body.
use super::{collapse, ids, is_fold, line_count, summary_label, walk, walk_mut, FoldMutation};
use crate::config::{Provider, SummarizeConfig};
use crate::protocol::{FileChange, Pairing, Problem, Source};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::sync::Semaphore;

const SYSTEM: &str =
    "You rewrite regions of a source file as terse Python-style pseudocode for a diff \
viewer that shows the pseudocode in place of the collapsed region. The user supplies \
one numbered source file and a list of folds, each with an id and 1-based line range. \
For each fold, write pseudocode covering only that fold's lines: keep the control flow \
and the names that matter, drop types, error plumbing and boilerplate. Aim for about one \
pseudocode line per five source lines, between one and eight lines per fold. Reply with \
one {id, pseudocode} object per fold.";

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

/// One fold to summarize: its region id and 1-based inclusive line range.
struct Request {
    id: u32,
    first_line: u32,
    last_line: u32,
}

#[derive(Deserialize)]
struct Answer {
    id: u32,
    pseudocode: String,
}

impl Summarizer {
    /// `None` when no API key resolves: the summarizer is then simply off.
    pub(crate) fn new(
        config: &SummarizeConfig,
        min_lines: usize,
    ) -> crate::git::Result<Option<Self>> {
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
            min_lines,
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
                format!(
                    "- fold {}: lines {}-{}",
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
    /// failure is a run-level problem.
    fn complete(
        &self,
        path: &str,
        language: Option<&str>,
        src: &str,
        folds: &[Request],
    ) -> Result<BTreeMap<u32, String>, Problem> {
        let body = json!({
            "systemInstruction": {"parts": [{"text": SYSTEM}]},
            "contents": [{"role": "user", "parts": [{"text": self.prompt(path, language, src, folds)}]}],
            "generationConfig": {
                "temperature": 0,
                "maxOutputTokens": 160 * folds.len() + 100,
                "thinkingConfig": {"thinkingBudget": 0},
                "responseMimeType": "application/json",
                "responseSchema": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": {"id": {"type": "INTEGER"}, "pseudocode": {"type": "STRING"}},
                        "required": ["id", "pseudocode"],
                    },
                },
            },
        });
        let url = self.url();
        let problem = |code: &str, message: String| Problem {
            code: code.to_owned(),
            message: format!("{}: {message}", self.config.model),
        };
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
                            .map_err(|error| problem("summarizer_failed", error.to_string()));
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
                        return Err(problem(
                            "summarizer_failed",
                            format!(
                                "HTTP {status} {}",
                                detail.chars().take(200).collect::<String>()
                            ),
                        ));
                    }
                    Err(error)
                        if error.is_timeout() || error.is_connect() || error.is_request() =>
                    {
                        error.to_string()
                    }
                    Err(error) => return Err(problem("summarizer_failed", error.to_string())),
                };
                if attempt >= self.config.retries {
                    return Err(problem(
                        "summarizer_failed",
                        format!("{retry} after {} attempts", attempt + 1),
                    ));
                }
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(250 * (1 << attempt.min(6)))).await;
            }
        })?;
        let content = text["candidates"][0]["content"]["parts"]
            .as_array()
            .and_then(|parts| parts.last())
            .and_then(|part| part["text"].as_str())
            .ok_or_else(|| problem("summarizer_failed", "no text in the response".to_owned()))?;
        let answers: Vec<Answer> = serde_json::from_str(content)
            .map_err(|error| problem("summarizer_failed", format!("{error}: {content}")))?;
        let mut texts = BTreeMap::new();
        for answer in answers {
            if !folds.iter().any(|fold| fold.id == answer.id) {
                return Err(problem(
                    "summarizer_failed",
                    format!("answered for unknown fold {}", answer.id),
                ));
            }
            if !answer.pseudocode.trim().is_empty() {
                texts.insert(answer.id, answer.pseudocode.trim().to_owned());
            }
        }
        Ok(texts)
    }
}

/// New function bodies on the after side of at least `min_lines` lines.
pub(crate) fn select(sides: &Pairing<Source>, min_lines: usize) -> Vec<(u32, u32, u32)> {
    let Some(rhs) = sides.rhs() else {
        return Vec::new();
    };
    let lhs_ids = sides.lhs().map(|lhs| ids(&lhs.regions)).unwrap_or_default();
    let mut selected = Vec::new();
    walk(&rhs.regions, &mut |region| {
        if is_fold(region)
            && region.tags.iter().any(|tag| tag == "body")
            && !lhs_ids.contains(&region.id)
            && line_count(region) >= min_lines
        {
            let lines = region.range.lines();
            selected.push((region.id, lines.start + 1, lines.end));
        }
    });
    selected
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
    fn apply(&self, file: &FileChange, sides: &mut Pairing<Source>) -> Result<(), Problem> {
        let folds: Vec<Request> = select(sides, self.min_lines)
            .into_iter()
            .map(|(id, first_line, last_line)| Request {
                id,
                first_line,
                last_line,
            })
            .collect();
        if folds.is_empty() {
            return Ok(());
        }
        let path = file
            .file
            .rhs()
            .map(|side| side.path.as_str())
            .unwrap_or_default();
        let language = file.language.as_deref();
        let texts = {
            let rhs = sides.rhs().expect("selection found rhs regions");
            self.complete(path, language, &rhs.text, &folds)?
        };
        let (Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs }) = sides else {
            unreachable!("selection found rhs regions");
        };
        walk_mut(&mut rhs.regions, &mut |region| {
            if let Some(text) = texts.get(&region.id) {
                collapse(region, summary_label(language, text));
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
        let params = Config::from_toml("").unwrap().compile().unwrap();
        let result = crate::summary::DiffResult::from_sources_with_options(
            path,
            before,
            after,
            &params,
            &crate::options::DisplayOptions::default(),
            &crate::options::DiffOptions::default(),
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
                context_lines: 3,
                syntax: (Vec::new(), Vec::new()),
            },
        )
        .unwrap();
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
        walk(&sides.rhs().unwrap().regions, &mut |region| {
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
            ..SummarizeConfig::default()
        };
        Summarizer::new(&config, 3).unwrap().unwrap()
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
    fn summaries_collapse_selected_folds_with_a_pseudocode_comment() {
        let (file, mut sides) = project("a.py", "", LARGE);
        let id = select(&sides, 3)[0].0;
        let (endpoint, server) = serve(vec![(200, gemini_answer(&[(id, "call a, b, c")]))]);
        summarizer(&endpoint, 0).apply(&file, &mut sides).unwrap();
        let bodies = server.join().unwrap();
        assert!(bodies[0].contains("thinkingBudget"));
        assert!(bodies[0].contains(&format!("fold {id}: lines 2-4")));
        let rhs = sides.rhs().unwrap();
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
    fn hard_failures_and_exhausted_retries_are_problems() {
        let (file, mut sides) = project("a.py", "", LARGE);
        let (endpoint, server) = serve(vec![(400, "{\"error\": \"bad key\"}".to_owned())]);
        let problem = summarizer(&endpoint, 3)
            .apply(&file, &mut sides)
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(problem.code, "summarizer_failed");
        assert!(problem.message.contains("HTTP 400"), "{}", problem.message);
        let (endpoint, server) = serve(vec![(500, "{}".to_owned()), (500, "{}".to_owned())]);
        let problem = summarizer(&endpoint, 1)
            .apply(&file, &mut sides)
            .unwrap_err();
        server.join().unwrap();
        assert!(
            problem.message.contains("after 2 attempts"),
            "{}",
            problem.message
        );
        assert!(!sides.rhs().unwrap().regions[0].visibility.collapsed);
    }

    #[test]
    fn small_files_never_call_the_model() {
        let (file, mut sides) = project("a.py", "", "def h():\n    e()\n");
        let config = SummarizeConfig {
            api_key: Some("k".to_owned()),
            endpoint: Some("http://127.0.0.1:1".to_owned()),
            ..SummarizeConfig::default()
        };
        Summarizer::new(&config, 3)
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
            assert!(Summarizer::new(&config, 3).unwrap().is_none());
        }
    }
}
