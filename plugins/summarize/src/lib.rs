//! The summarizer: large new function bodies become short pseudocode, shown
//! in place of the collapsed body.
//!
//! It needs an API key: `new` fails without one, naming how to set it or
//! turn the plugin off, which is why the bundled configuration ships it off.
//! The plugin holds its HTTP client and the runtime every request runs on,
//! and at most `max_concurrency` requests are in flight at once across
//! files.
use diffr_plugin_sdk::anyhow::{self, anyhow, Context as _};
use diffr_plugin_sdk::{
    docstring_of, export, has_tag, is_fold, line_count, one_sided, walk, Draft, FileEntry, Move,
    Node, OtherSide, Pairing, Plugin, Region, Source,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use tokio::runtime::Runtime;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::Semaphore;
#[cfg(target_arch = "wasm32")]
mod http;

/// The plugin's name, and the tags its queries set: a function body, and a
/// test body, which is never summarized.
const PLUGIN: &str = "summarize";
const FUNCTION: &str = "summarize:function";
const TEST: &str = "summarize:test";

/// The plugin's options, as `plugins/summarize/plugin.toml` declares them.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub provider: Provider,
    pub model: String,
    pub min_lines: usize,
    pub api_key: Option<String>,
    pub endpoint: Option<String>,
    pub request_timeout_ms: u64,
    pub max_concurrency: usize,
    pub retries: u32,
    /// The system instruction sent with every request.
    pub system_prompt: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Gemini,
}

const DEFAULT_ENDPOINT: &str = "https://generativelanguage.googleapis.com";

/// The summarizer: its options, its API key and endpoint, its HTTP client,
/// the runtime its requests run on, and the requests in flight across files.
pub struct Summarize {
    options: Options,
    api_key: String,
    endpoint: String,
    #[cfg(not(target_arch = "wasm32"))]
    client: reqwest::Client,
    #[cfg(not(target_arch = "wasm32"))]
    runtime: Runtime,
    #[cfg(not(target_arch = "wasm32"))]
    limit: Semaphore,
}

/// One fold to summarize: its region id, 1-based inclusive line range, and
/// its docstring region and that region's text when it has one.
struct Request {
    id: u32,
    first_line: u32,
    last_line: u32,
    docstring: Option<u32>,
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

impl Summarize {
    fn url(&self) -> String {
        match self.options.provider {
            Provider::Gemini => format!(
                "{}/v1beta/models/{}:generateContent",
                self.endpoint.trim_end_matches('/'),
                self.options.model
            ),
        }
    }

    fn prompt(&self, path: &str, src: &str, folds: &[Request]) -> String {
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
            "File {path}:\n\n{}\n\nFolds:\n{}",
            numbered.join("\n"),
            ranges.join("\n")
        )
    }

    /// One request per file, retried on transient failures. Any other
    /// failure is a run-level failure.
    fn complete(
        &self,
        path: &str,
        src: &str,
        folds: &[Request],
    ) -> anyhow::Result<BTreeMap<u32, Summary>> {
        let body = json!({
            "systemInstruction": {"parts": [{"text": self.options.system_prompt}]},
            "contents": [{"role": "user", "parts": [{"text": self.prompt(path, src, folds)}]}],
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
        let failed = |message: String| anyhow!("{}: {message}", self.options.model);
        #[cfg(not(target_arch = "wasm32"))]
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
                if attempt >= self.options.retries {
                    return Err(failed(format!("{retry} after {} attempts", attempt + 1)));
                }
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(250 * (1 << attempt.min(6)))).await;
            }
        })?;
        #[cfg(target_arch = "wasm32")]
        let text: serde_json::Value = {
            let mut attempt = 0;
            loop {
                let result = http::post(
                    &url,
                    &self.api_key,
                    &body.to_string(),
                    self.options.request_timeout_ms,
                );
                let retry = match result {
                    Ok((status, body)) if (200..300).contains(&status) => {
                        break serde_json::from_slice(&body)
                            .map_err(|error| failed(error.to_string()))?;
                    }
                    Ok((status, _)) if status == 429 || status >= 500 => format!("HTTP {status}"),
                    Ok((status, body)) => {
                        return Err(failed(format!(
                            "HTTP {status} {}",
                            String::from_utf8_lossy(&body)
                                .chars()
                                .take(200)
                                .collect::<String>()
                        )))
                    }
                    Err(error) => error.to_string(),
                };
                if attempt >= self.options.retries {
                    return Err(failed(format!("{retry} after {} attempts", attempt + 1)));
                }
                attempt += 1;
                std::thread::sleep(Duration::from_millis(250 * (1 << attempt.min(6))));
            }
        };
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
/// function folds with no line inside them paired with the before side (see
/// `one_sided`), so a body that only moved, or a file diffed by line whose
/// bodies still align, is not new. Only the outermost qualifying body is
/// taken, never one nested inside it; test bodies and folds that already
/// start collapsed are skipped. Each is its id, its 1-based inclusive line
/// range, and its docstring's id. Selection runs before this plugin links
/// any docstring, so a docstring matched across sides never makes the new
/// body under it look paired.
pub fn select(sides: &Pairing<Source>, min_lines: usize) -> Vec<(u32, u32, u32, Option<u32>)> {
    let (lhs, rhs) = match sides {
        Pairing::Both { lhs, rhs } => (OtherSide::of(&lhs.regions), rhs),
        Pairing::RightOnly { rhs } => (OtherSide::default(), rhs),
        Pairing::LeftOnly { .. } => return Vec::new(),
    };
    let mut selected = Vec::new();
    fn visit(
        rhs: &Source,
        regions: &[Region],
        lhs: &OtherSide,
        min_lines: usize,
        selected: &mut Vec<(u32, u32, u32, Option<u32>)>,
    ) {
        for region in regions {
            if is_fold(region)
                && has_tag(region, FUNCTION)
                && !has_tag(region, TEST)
                && !region.visibility.collapsed
                && one_sided(region, lhs)
                && line_count(region) >= min_lines
            {
                let lines = region.range.lines();
                selected.push((
                    region.id,
                    lines.start + 1,
                    lines.end,
                    docstring_of(rhs, region, PLUGIN),
                ));
                continue;
            }
            if let Node::Fold { children } = &region.node {
                visit(rhs, children, lhs, min_lines, selected);
            }
        }
    }
    visit(rhs, &rhs.regions, &lhs, min_lines, &mut selected);
    selected
}

/// The text of the docstring region with this id, with comment markers and
/// quotes stripped, or `None` when it says nothing.
fn documentation(source: &Source, docstring: u32) -> Option<String> {
    let lines: Vec<&str> = source.text.split_terminator('\n').collect();
    let mut words = Vec::new();
    walk(&source.regions, &mut |region| {
        if region.id != docstring {
            return;
        }
        for line in region.range.lines() {
            let stripped = strip_markers(lines[line as usize]);
            if !stripped.is_empty() {
                words.push(stripped.to_owned());
            }
        }
    });
    let text = words.join(" ");
    (!text.is_empty()).then_some(text)
}

fn strip_markers(line: &str) -> &str {
    let mut text = line.trim();
    for prefix in [
        "///", "//!", "//", "/**", "/*", "*/", "*", "#", "--", ";;", ";", "%",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim();
            break;
        }
    }
    for quote in ["\"\"\"", "'''"] {
        text = text
            .trim_start_matches(quote)
            .trim_end_matches(quote)
            .trim();
    }
    text.trim_end_matches("*/").trim()
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

/// The API key: the `api_key` option, or else `GEMINI_API_KEY`, or else
/// `GOOGLE_API_KEY`, the first that is set and not empty.
fn resolve_key(config: &Options) -> anyhow::Result<String> {
    let set = |key: &String| !key.is_empty();
    if let Some(key) = config.api_key.clone().filter(set) {
        return Ok(key);
    }
    for variable in ["GEMINI_API_KEY", "GOOGLE_API_KEY"] {
        if let Some(key) = std::env::var_os(variable) {
            let key = key
                .into_string()
                .map_err(|_| anyhow!("{variable} is not valid UTF-8"))?;
            if set(&key) {
                return Ok(key);
            }
        }
    }
    anyhow::bail!(
        "no API key: set plugins.bundled.summarize.api_key, or GEMINI_API_KEY or GOOGLE_API_KEY in the environment, or turn the summarizer off with plugins.bundled.summarize.enabled = false"
    )
}

impl Plugin for Summarize {
    type Options = Options;

    fn new(options: Options) -> anyhow::Result<Self> {
        let api_key = resolve_key(&options)?;
        #[cfg(not(target_arch = "wasm32"))]
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(options.request_timeout_ms))
            .build()?;
        #[cfg(not(target_arch = "wasm32"))]
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("diffr-summarizer")
            .enable_all()
            .build()?;
        Ok(Self {
            api_key,
            endpoint: options
                .endpoint
                .clone()
                .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned()),
            #[cfg(not(target_arch = "wasm32"))]
            client,
            #[cfg(not(target_arch = "wasm32"))]
            runtime,
            #[cfg(not(target_arch = "wasm32"))]
            limit: Semaphore::new(options.max_concurrency),
            options,
        })
    }

    fn queries(&self) -> anyhow::Result<Vec<diffr_plugin_sdk::QuerySource>> {
        Ok(vec![
            diffr_plugin_sdk::QuerySource {
                language: "rust".into(),
                name: "builtin:summarize/queries/rust.scm".into(),
                text: include_str!("../queries/rust.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "python".into(),
                name: "builtin:summarize/queries/python.scm".into(),
                text: include_str!("../queries/python.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "go".into(),
                name: "builtin:summarize/queries/go.scm".into(),
                text: include_str!("../queries/go.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "javascript".into(),
                name: "builtin:summarize/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "javascriptjsx".into(),
                name: "builtin:summarize/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "typescript".into(),
                name: "builtin:summarize/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "typescripttsx".into(),
                name: "builtin:summarize/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
        ])
    }

    fn classify(&self, _file: &FileEntry) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn mutate(&self, file: &FileEntry, sides: &Pairing<Source>) -> anyhow::Result<Vec<Move>> {
        let selected = select(sides, self.options.min_lines);
        let (Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs }) = &sides else {
            return Ok(Vec::new());
        };
        let folds: Vec<Request> = selected
            .into_iter()
            .map(|(id, first_line, last_line, docstring)| Request {
                id,
                first_line,
                last_line,
                docstring,
                doc: docstring.and_then(|docstring| documentation(rhs, docstring)),
            })
            .collect();
        if folds.is_empty() {
            return Ok(Vec::new());
        }
        let lines: Vec<&str> = rhs.text.split_terminator('\n').collect();
        let mut texts = self
            .complete(file.path(), &rhs.text, &folds)
            .context("summarizer")?;
        texts.retain(|id, summary| {
            let fold = folds
                .iter()
                .find(|fold| fold.id == *id)
                .expect("answered fold");
            let body = &lines[fold.first_line as usize - 1..fold.last_line as usize];
            compresses(&summary.pseudocode, body)
        });
        // Collapse every summarized body first, then link each to its
        // docstring: a summary and its docstring are one thing.
        let mut draft = Draft::new(sides);
        let mut links = Vec::new();
        for (id, summary) in texts {
            let text = match &summary.quote {
                Some(quote) => format!("{quote}\n{}", summary.pseudocode),
                None => summary.pseudocode.clone(),
            };
            draft.collapse(id, text)?;
            let fold = folds
                .iter()
                .find(|fold| fold.id == id)
                .expect("answered fold");
            if let Some(docstring) = fold.docstring {
                links.push([id, docstring]);
            }
        }
        for link in links {
            draft.link(&link)?;
        }
        Ok(draft.into_moves())
    }
}

export!("summarize", Summarize);
