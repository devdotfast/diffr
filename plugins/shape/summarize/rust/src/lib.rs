//! The summarizer: large new function bodies become short pseudocode, shown
//! in place of the collapsed body.
//!
//! It needs an API key for most providers: `new` fails without one, naming
//! how to set it or turn the plugin off, which is why the bundled
//! configuration ships it off.
//! Each selected body is summarized and edited in its own asynchronous callback.
use anyhow::anyhow;
use diffr_plugin_sdk::prelude::*;
use serde::Deserialize;
use std::time::Duration;
mod http;
mod provider;
pub use provider::{Details, Provider};

/// The plugin's name, and the tags its queries set: a function body, and a
/// test body, which can be summarized independently of whether it is new.
const FUNCTION: &str = "summarize:function";
const TEST: &str = "summarize:test";

/// The plugin's options, as `plugins/summarize/plugin.toml` declares them.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub provider: Provider,
    pub provider_details: Details,
    pub model: String,
    pub min_lines: usize,
    pub tests: bool,
    pub test_min_lines: usize,
    pub api_key: Option<String>,
    pub endpoint: Option<String>,
    pub request_timeout_ms: u64,
    pub retries: u32,
    /// The system instruction sent with every request.
    pub system_prompt: String,
}

/// The summarizer's options, API key and endpoint.
pub struct Summarize {
    options: Options,
    api_key: Option<String>,
    endpoint: String,
}

/// One fold to summarize: its region id, 1-based inclusive line range, and
/// its documentation text when it has any.
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

impl Summarize {
    fn prompt(&self, path: &str, src: &str, fold: &Request) -> String {
        let numbered = src
            .split_terminator('\n')
            .enumerate()
            .map(|(index, line)| format!("{:5} | {line}", fold.first_line as usize + index))
            .collect::<Vec<_>>()
            .join("\n");
        let doc = fold
            .doc
            .as_ref()
            .map(|doc| format!("\n  doc: {doc}"))
            .unwrap_or_default();
        format!(
            "File {path}:\n\n{numbered}\n\nFold:\n- fold {}: lines {}-{}{doc}",
            fold.id, fold.first_line, fold.last_line
        )
    }

    /// One request per selected body, retried on transient failures. Any other
    /// failure is a run-level failure.
    async fn complete(
        &self,
        path: &str,
        src: &str,
        fold: &Request,
    ) -> anyhow::Result<Option<Summary>> {
        let provider = self.options.provider;
        let body = provider.body(
            &self.options.model,
            &self.options.system_prompt,
            &self.prompt(path, src, fold),
            800,
        );
        let url = provider.url(&self.endpoint, &self.options.model);
        let headers = provider.headers(self.api_key.as_deref());
        let failed = |message: String| anyhow!("{}: {message}", self.options.model);
        let text: serde_json::Value = {
            let mut attempt = 0;
            loop {
                let result = http::post(
                    &url,
                    &headers,
                    &body.to_string(),
                    self.options.request_timeout_ms,
                )
                .await;
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
                http::sleep(Duration::from_millis(250 * (1 << attempt.min(6)))).await;
            }
        };
        let content = provider
            .text(&text)
            .ok_or_else(|| failed("no text in the response".to_owned()))?;
        let answers: Vec<Answer> = provider::answers(content)
            .ok_or_else(|| failed(format!("no summaries in the answer: {content}")))?;
        if answers.len() > 1 {
            return Err(failed("expected at most one summary".into()));
        }
        let Some(answer) = answers.into_iter().next() else {
            return Ok(None);
        };
        if answer.id != fold.id {
            return Err(failed(format!("answered for unknown fold {}", answer.id)));
        }
        Ok((!answer.pseudocode.trim().is_empty()).then(|| Summary {
            quote: quoted(fold.doc.as_deref(), &answer.summary),
            pseudocode: answer.pseudocode.trim().to_owned(),
        }))
    }
}

/// The text of the docstring region with this id, with comment markers and
/// quotes stripped, or `None` when it says nothing.
fn documentation(text: &str) -> Option<String> {
    let words: Vec<_> = text
        .split_terminator('\n')
        .map(strip_markers)
        .filter(|line| !line.is_empty())
        .collect();
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
/// dropped; the body retains its original visibility.
fn compresses(summary: &str, body: &[&str]) -> bool {
    let summary_lines = summary
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    let body_lines = body.iter().filter(|line| !line.trim().is_empty()).count();
    summary_lines * 2 <= body_lines
}

/// The API key: the `api_key` option, or else the first of the provider's
/// environment variables that is set and not empty. `None` only where the
/// provider can go without one.
fn resolve_key(config: &Options, custom_endpoint: bool) -> anyhow::Result<Option<String>> {
    let set = |key: &String| !key.is_empty();
    if let Some(key) = config.api_key.clone().filter(set) {
        return Ok(Some(key));
    }
    let variables = &config.provider_details.key_variables;
    for variable in variables {
        if let Some(key) = std::env::var_os(variable) {
            let key = key
                .into_string()
                .map_err(|_| anyhow!("{variable} is not valid UTF-8"))?;
            if set(&key) {
                return Ok(Some(key));
            }
        }
    }
    if config.provider_details.keyless_custom_endpoint && custom_endpoint {
        return Ok(None);
    }
    anyhow::bail!(
        "no API key: set plugins.bundled.summarize.api_key, or {} in the environment, or turn the summarizer off with plugins.bundled.summarize.enabled = false",
        variables.join(" or ")
    )
}

impl Guest for Summarize {
    type Plugin = Self;
}

impl GuestPlugin for Summarize {
    fn new(options: String) -> Result<Self, String> {
        let options: Options =
            serde_json::from_str(&options).map_err(|e| format!("invalid options: {e}"))?;
        let endpoint = options
            .endpoint
            .clone()
            .filter(|endpoint| !endpoint.is_empty());
        let api_key = resolve_key(&options, endpoint.is_some()).map_err(|e| format!("{e:#}"))?;
        Ok(Self {
            api_key,
            endpoint: endpoint.unwrap_or_else(|| options.provider_details.endpoint.clone()),
            options,
        })
    }

    async fn visit(&self, cursor: &Cursor, phase: Visit) -> Result<bool, String> {
        if phase == Visit::Post {
            return Ok(true);
        }
        let NodeView::Region(RegionView {
            side: Side::Rhs,
            data,
            ..
        }) = cursor.get(cursor.id())?
        else {
            return Ok(cursor.id() == ROOT);
        };
        if !self.eligible(cursor, &data)? {
            return Ok(true);
        }
        let text = cursor.text(data.id)?;
        let docstring = cursor.related(data.id, "documentation")?.into_iter().next();
        let doc = docstring
            .map(|id| cursor.text(id))
            .transpose()?
            .as_deref()
            .and_then(documentation);
        let lines = data.range.start.line..data.range.end.line;
        let request = Request {
            id: data.id,
            first_line: lines.start + 1,
            last_line: lines.end,
            doc,
        };
        let file = cursor.file();
        let path = match &file.file {
            FileSides::Both((_, rhs)) | FileSides::RightOnly(rhs) => &rhs.path,
            FileSides::LeftOnly(lhs) => &lhs.path,
        };
        if let Some(summary) = self
            .complete(path, &text, &request)
            .await
            .map_err(|e| format!("summarizer: {e:#}"))?
        {
            if compresses(
                &summary.pseudocode,
                &text.split_terminator('\n').collect::<Vec<_>>(),
            ) {
                let label = match summary.quote {
                    Some(quote) => format!("{quote}\n{}", summary.pseudocode),
                    None => summary.pseudocode,
                };
                cursor.set_collapsed(data.id, true)?;
                cursor.set_label(data.id, Some(&label))?;
                if let Some(docstring) = docstring {
                    cursor.link(&[data.id, docstring])?;
                }
            }
        }
        Ok(false)
    }
}

impl Summarize {
    fn eligible(&self, cursor: &Cursor, data: &Region) -> Result<bool, String> {
        if !matches!(data.kind, Kind::Fold) {
            return Ok(false);
        }
        let count = (data.range.end.line - data.range.start.line) as usize;
        Ok(if data.tags.iter().any(|tag| tag == TEST) {
            self.options.tests && count >= self.options.test_min_lines
        } else {
            data.tags.iter().any(|tag| tag == FUNCTION)
                && !data.visibility.collapsed
                && count >= self.options.min_lines
                && cursor.is_one_sided(data.id)?
        })
    }
}

export!(Summarize);
