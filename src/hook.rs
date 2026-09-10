//! Trusted fold-summary hook: a JSON-RPC 2.0 server that diffr starts once per
//! session and calls over loopback HTTP.
//!
//! The hook receives its port in `DIFFR_HOOK_PORT` and the diffed repository in
//! `DIFFR_WORKSPACE`. Each call carries one file and its large novel folds on
//! the after side. Workers block on their own call while the client multiplexes
//! every in-flight request on a small tokio runtime, so files still stream out
//! as each worker finishes.
use crate::config::HookConfig;
use crate::parse::folds::FoldMatch;
use crate::review::wire;
use crate::summary::{DiffResult, FileContent, FileFormat};
use jsonrpsee::core::ClientError;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::proc_macros::rpc;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Serialize)]
pub(crate) struct RequestFold {
    /// Index into the file's rhs folds.
    id: usize,
    range: Value,
    tags: Vec<String>,
    placeholder: String,
}

/// The interface every hook implements. Params are sent by name; the result
/// maps fold ids, as strings, to replacement text.
#[rpc(client)]
trait FoldHook {
    #[method(name = "summarize", param_kind = map)]
    async fn summarize(
        &self,
        path: String,
        language: Option<String>,
        src: String,
        folds: Vec<RequestFold>,
    ) -> jsonrpsee::core::RpcResult<BTreeMap<String, String>>;
}

pub(crate) struct Hook {
    config: HookConfig,
    child: Mutex<Child>,
    runtime: tokio::runtime::Runtime,
    client: HttpClient,
}

impl Hook {
    pub(crate) fn spawn(config: &HookConfig, workspace: &Path) -> crate::git::Result<Self> {
        let port = free_port()?;
        let mut child = Command::new(&config.command[0])
            .args(&config.command[1..])
            .current_dir(&config.dir)
            .env("DIFFR_HOOK_PORT", port.to_string())
            .env("DIFFR_WORKSPACE", workspace)
            .stdin(Stdio::null())
            // Stdout belongs to diffr's own stream; hooks log to stderr.
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("could not start fold hook {:?}: {error}", config.command))?;
        if let Err(error) = await_listening(&mut child, port, config.startup_timeout_ms) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("diffr-hook-client")
            .enable_all()
            .build()?;
        let client = runtime.block_on(async {
            HttpClientBuilder::default()
                .request_timeout(Duration::from_millis(config.timeout_ms))
                .build(format!("http://127.0.0.1:{port}"))
        })?;
        Ok(Self {
            config: config.clone(),
            child: Mutex::new(child),
            runtime,
            client,
        })
    }

    /// Fill in summaries for this file's qualifying folds, blocking on the hook.
    /// Files without qualifying folds never reach the hook.
    pub(crate) fn summarize(&self, diff: &mut DiffResult) -> Result<(), String> {
        let selected: Vec<usize> = diff
            .rhs_folds
            .iter()
            .enumerate()
            .filter(|(_, fold)| self.qualifies(fold))
            .map(|(index, _)| index)
            .collect();
        if selected.is_empty() {
            return Ok(());
        }
        let FileContent::Text(src) = &diff.rhs_src else {
            return Ok(());
        };
        let language = match &diff.file_format {
            FileFormat::SupportedLanguage(language) => {
                Some(crate::parse::guess_language::language_name(*language).to_owned())
            }
            _ => None,
        };
        let folds = selected
            .iter()
            .map(|&index| {
                let fold = &diff.rhs_folds[index];
                RequestFold {
                    id: index,
                    range: wire::range(&fold.range),
                    tags: fold.tags.clone(),
                    placeholder: fold.placeholder.clone(),
                }
            })
            .collect();
        let texts = self
            .runtime
            .block_on(FoldHookClient::summarize(
                &self.client,
                diff.display_path.clone(),
                language,
                src.clone(),
                folds,
            ))
            .map_err(|error| match error {
                ClientError::Call(error) => format!("fold hook reported: {}", error.message()),
                ClientError::RequestTimeout => {
                    format!("fold hook timed out after {}ms", self.config.timeout_ms)
                }
                other => format!("fold hook: {other}"),
            })?;
        for (key, text) in texts {
            let index: usize = key
                .parse()
                .ok()
                .filter(|index| selected.contains(index))
                .ok_or_else(|| format!("fold hook answered for unknown fold {key:?}"))?;
            diff.rhs_folds[index].summary = Some(text);
        }
        Ok(())
    }

    fn qualifies(&self, fold: &crate::parse::folds::Fold) -> bool {
        if !matches!(fold.match_kind, FoldMatch::Novel) {
            return false;
        }
        let lines = (fold.range.end.line.0 - fold.range.start.line.0 + 1) as usize;
        if lines < self.config.min_lines {
            return false;
        }
        match &self.config.tags {
            Some(tags) => fold.tags.iter().any(|tag| tags.contains(tag)),
            None => true,
        }
    }
}

/// Reserve a loopback port for the hook. The listener is released before the
/// hook starts, which is the usual small race on a single machine.
fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

/// Poll until the hook accepts connections, or fail early if it exits.
fn await_listening(child: &mut Child, port: u16, startup_timeout_ms: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(startup_timeout_ms);
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    loop {
        if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
            return Ok(());
        }
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(format!("fold hook exited during startup with {status}"));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "fold hook did not listen on port {port} within {startup_timeout_ms}ms"
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::sync::Arc;

    fn hook(mode: &str, timeout_ms: u64) -> crate::git::Result<Hook> {
        let config = HookConfig {
            dir: env!("CARGO_MANIFEST_DIR").into(),
            command: vec![
                "python3".into(),
                "tests/hooks/rpc_server.py".into(),
                mode.into(),
            ],
            tags: Some(vec!["body".into()]),
            min_lines: 2,
            timeout_ms,
            startup_timeout_ms: 10_000,
        };
        Hook::spawn(&config, Path::new("."))
    }

    fn diff(rhs: &str) -> DiffResult {
        let params = Config::from_toml("").unwrap().compile().unwrap();
        DiffResult::from_sources_with_options(
            "file.py",
            "",
            rhs,
            &params,
            &crate::options::DisplayOptions::default(),
            &crate::options::DiffOptions::default(),
        )
    }

    const LARGE: &str = "def f():\n    a()\n    b()\n    c()\n\ndef g():\n    d()\n";

    #[test]
    fn summaries_land_on_selected_folds_only() {
        let hook = hook("first", 5000).unwrap();
        let mut result = diff(LARGE);
        assert_eq!(result.rhs_folds.len(), 2);
        hook.summarize(&mut result).unwrap();
        assert_eq!(result.rhs_folds[0].summary.as_deref(), Some("summary of f"));
        assert_eq!(result.rhs_folds[1].summary, None);
        assert!(result.lhs_folds.iter().all(|fold| fold.summary.is_none()));
    }

    #[test]
    fn concurrent_calls_share_one_client() {
        let hook = Arc::new(hook("echo", 5000).unwrap());
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let hook = Arc::clone(&hook);
                std::thread::spawn(move || {
                    let mut result = diff(LARGE);
                    hook.summarize(&mut result).unwrap();
                    result.rhs_folds[0].summary.clone()
                })
            })
            .collect();
        for worker in workers {
            assert_eq!(worker.join().unwrap().as_deref(), Some("pseudo Body"));
        }
    }

    #[test]
    fn small_or_untagged_folds_never_reach_the_hook() {
        let hook = hook("error", 5000).unwrap();
        let mut result = diff("import os\nimport sys\n");
        hook.summarize(&mut result).unwrap();
        let mut result = diff("def f():\n    a()\n");
        hook.summarize(&mut result).unwrap();
    }

    #[test]
    fn startup_failures_are_reported() {
        let error = hook("exit", 5000)
            .err()
            .expect("exit must fail")
            .to_string();
        assert!(error.contains("exited during startup"), "{error}");
    }

    #[test]
    fn call_failures_are_reported_without_losing_the_diff() {
        let mut result = diff(LARGE);
        let error = hook("slow", 200)
            .unwrap()
            .summarize(&mut result)
            .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        let error = hook("error", 5000)
            .unwrap()
            .summarize(&mut result)
            .unwrap_err();
        assert!(error.contains("declined"), "{error}");
        let error = hook("bad", 5000)
            .unwrap()
            .summarize(&mut result)
            .unwrap_err();
        assert!(error.starts_with("fold hook:"), "{error}");
        assert!(result.rhs_folds.iter().all(|fold| fold.summary.is_none()));
    }
}
