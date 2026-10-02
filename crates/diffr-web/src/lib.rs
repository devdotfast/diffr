//! diffr in the browser. A [`Differ`] holds what the CLI builds once per
//! run (the configuration, the bundled plugins and their compiled fold
//! queries); [`Differ::diff`] turns the two sides of one changed file into
//! its `file` record of the wire protocol, shaped by the plugins exactly as
//! the CLI shapes it. Fetching the sources is the page's job.
use diffr_cli::config::{Config, DiffConfig};
use diffr_cli::pairing::Pairing;
use diffr_cli::params::Params;
use diffr_cli::plugin::Pipeline;
use diffr_cli::protocol::stream::{self, Options};
use diffr_cli::protocol::{Event, FileChange, FileRef, FileStatus};
use serde::{Deserialize, Serialize};
use std::path::Path;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Differ {
    params: Params,
    pipeline: Pipeline,
    limits: DiffConfig,
}

/// One side of a changed file, as the page names it.
#[derive(Deserialize)]
struct Side {
    path: String,
    #[serde(default)]
    oid: String,
    #[serde(default)]
    mode: String,
    /// The file's text; absent for the side a change does not have.
    text: Option<String>,
}

#[derive(Deserialize)]
struct Request {
    status: FileStatus,
    lhs: Option<Side>,
    rhs: Option<Side>,
    #[serde(default)]
    syntax: bool,
}

#[derive(Serialize)]
struct Response {
    entry: FileChange,
    event: Event,
}

#[wasm_bindgen]
impl Differ {
    /// A differ for `config`, the text of a `config.toml`, or the bundled
    /// defaults when it is absent.
    #[wasm_bindgen(constructor)]
    pub fn new(config: Option<String>) -> Result<Differ, JsError> {
        console_error_panic_hook::set_once();
        let config = match config {
            Some(text) => Config::from_toml_in(&text, Path::new("")),
            None => Ok(Config::default()),
        }
        .map_err(|error| JsError::new(&error.to_string()))?;
        let pipeline = Pipeline::from_config(&config.plugins, Path::new("."))
            .map_err(|error| JsError::new(&format!("{error:#}")))?;
        let limits = config.diff;
        let params = config
            .compile_with(&pipeline)
            .map_err(|error| JsError::new(&error.to_string()))?;
        Ok(Differ {
            params,
            pipeline,
            limits,
        })
    }

    /// Diff one changed file. `request` is JSON:
    /// `{"status": "modified", "lhs": {"path", "text"}, "rhs": {"path", "text"}}`,
    /// with `lhs` absent for an added file and `rhs` for a deleted one, and
    /// an optional `"syntax": true` for highlight spans. Returns JSON
    /// `{"entry": <manifest entry>, "event": <file record>}`.
    pub fn diff(&self, request: &str) -> Result<String, JsError> {
        let request: Request =
            serde_json::from_str(request).map_err(|error| JsError::new(&error.to_string()))?;
        let file_ref = |side: &Side| FileRef {
            path: side.path.clone(),
            oid: side.oid.clone(),
            mode: side.mode.clone(),
        };
        let file = match (&request.lhs, &request.rhs) {
            (Some(lhs), Some(rhs)) => Pairing::Both {
                lhs: file_ref(lhs),
                rhs: file_ref(rhs),
            },
            (Some(lhs), None) => Pairing::LeftOnly { lhs: file_ref(lhs) },
            (None, Some(rhs)) => Pairing::RightOnly { rhs: file_ref(rhs) },
            (None, None) => return Err(JsError::new("a changed file needs a side")),
        };
        fn text(side: &Option<Side>) -> Option<&str> {
            side.as_ref().and_then(|side| side.text.as_deref())
        }
        let (entry, event) = stream::file_from_sources(
            file,
            request.status,
            text(&request.lhs),
            text(&request.rhs),
            &self.params,
            &self.pipeline,
            &self.limits.options(false),
            Options {
                syntax: request.syntax,
                updates: false,
            },
        );
        serde_json::to_string(&Response { entry, event })
            .map_err(|error| JsError::new(&error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_modified_file_gets_a_shaped_record() {
        let differ = Differ::new(None).unwrap_or_else(|_| panic!("the bundled defaults load"));
        let before = "export function f(a: number) {\n  const x = a + 1;\n  return x * 2;\n}\n";
        let after =
            "export function f(a: number, b: number) {\n  const x = a + b;\n  return x * 2;\n}\n";
        let request = serde_json::json!({
            "status": "modified",
            "lhs": {"path": "src/a.ts", "text": before},
            "rhs": {"path": "src/a.ts", "text": after},
            "syntax": true,
        });
        let response: serde_json::Value = serde_json::from_str(
            &differ
                .diff(&request.to_string())
                .unwrap_or_else(|_| panic!("the file diffs")),
        )
        .unwrap();
        assert_eq!(response["entry"]["status"], "modified");
        let event = &response["event"];
        assert_eq!(event["type"], "file");
        assert_eq!(event["diff"]["type"], "text", "{event}");
        assert!(!event["diff"]["rhs"]["regions"].as_array().unwrap().is_empty());
        assert!(!event["diff"]["rhs"]["syntax"].as_array().unwrap().is_empty());
        assert_eq!(event["diff"]["stats"]["textual"]["added"], 2);
    }
}
