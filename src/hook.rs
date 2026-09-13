//! Trusted fold-summary hook: a JSON-RPC 2.0 server that diffr starts once per
//! session and calls over loopback HTTP. It is a fold mutation like the
//! built-in summarizer, and runs after it.
//!
//! The hook receives its port in `DIFFR_HOOK_PORT` and the diffed repository in
//! `DIFFR_WORKSPACE`. Each call carries one file and its large new fold
//! regions on the after side. Workers block on their own call while the
//! client multiplexes every in-flight request on a small tokio runtime, so
//! files still stream out as each worker finishes.
use crate::config::HookConfig;
use crate::hash::DftHashSet;
use crate::mutate::{
    collapse, ids, is_fold, line_count, summary_label, walk, walk_mut, Failure, FoldMutation,
};
use crate::pairing::Pairing;
use crate::protocol::{FileChange, Region, Source, SourceRange};
use anyhow::{anyhow, Context as _};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Serialize)]
pub(crate) struct RequestFold {
    /// The region id on the after side.
    id: u32,
    range: SourceRange,
    tags: Vec<String>,
    /// The label the region has before the hook answers, e.g. `Body`.
    placeholder: String,
}

#[derive(Serialize)]
struct Params<'a> {
    path: &'a str,
    language: Option<&'a str>,
    src: &'a str,
    folds: Vec<RequestFold>,
}

#[derive(Serialize)]
struct Request<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: Params<'a>,
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    result: Option<BTreeMap<String, String>>,
    #[serde(default)]
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    message: String,
}

pub(crate) struct Hook {
    config: HookConfig,
    min_lines: usize,
    child: Mutex<Child>,
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
    url: String,
    next_id: AtomicU64,
}

impl Hook {
    pub(crate) fn spawn(
        config: &HookConfig,
        default_min_lines: usize,
        workspace: &Path,
    ) -> anyhow::Result<Self> {
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
            .map_err(|error| anyhow!("could not start fold hook {:?}: {error}", config.command))?;
        if let Err(error) = await_listening(&mut child, port, config.startup_timeout_ms) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("diffr-hook-client")
            .enable_all()
            .build()?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()?;
        Ok(Self {
            config: config.clone(),
            min_lines: config.min_lines.unwrap_or(default_min_lines),
            child: Mutex::new(child),
            runtime,
            client,
            url: format!("http://127.0.0.1:{port}/"),
            next_id: AtomicU64::new(1),
        })
    }

    fn qualifies(&self, region: &Region, lhs_ids: &DftHashSet<u32>) -> bool {
        if !is_fold(region) || lhs_ids.contains(&region.alignment_id) {
            return false;
        }
        if line_count(region) < self.min_lines {
            return false;
        }
        match &self.config.tags {
            Some(tags) => region.tags.iter().any(|tag| tags.contains(tag)),
            None => !region.tags.is_empty(),
        }
    }

    fn call(&self, params: Params<'_>) -> anyhow::Result<BTreeMap<String, String>> {
        let request = Request {
            jsonrpc: "2.0",
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            method: "summarize",
            params,
        };
        self.runtime.block_on(async {
            let response = self
                .client
                .post(&self.url)
                .json(&request)
                .send()
                .await
                .map_err(|error| {
                    if error.is_timeout() {
                        anyhow!("timed out after {}ms", self.config.timeout_ms)
                    } else {
                        anyhow!(error)
                    }
                })?;
            let status = response.status();
            let body = response.text().await?;
            let response: Response = serde_json::from_str(&body)
                .with_context(|| format!("invalid response ({status})"))?;
            match (response.result, response.error) {
                (_, Some(error)) => Err(anyhow!("the hook reported: {}", error.message)),
                (Some(result), None) => Ok(result),
                (None, None) => Err(anyhow!("response has neither result nor error")),
            }
        })
    }
}

impl FoldMutation for Hook {
    /// Files without qualifying folds never reach the hook.
    fn apply(&self, file: &FileChange, sides: &mut Pairing<Source>) -> anyhow::Result<()> {
        let (lhs_ids, rhs) = match sides {
            Pairing::Both { lhs, rhs } => (ids(&lhs.regions), rhs),
            Pairing::RightOnly { rhs } => (DftHashSet::default(), rhs),
            Pairing::LeftOnly { .. } => return Ok(()),
        };
        let mut folds = Vec::new();
        walk(&rhs.regions, &mut |region| {
            if self.qualifies(region, &lhs_ids) {
                folds.push(RequestFold {
                    id: region.alignment_id,
                    range: region.range,
                    tags: region.tags.clone(),
                    placeholder: region.visibility.label.clone(),
                });
            }
        });
        if folds.is_empty() {
            return Ok(());
        }
        let selected: Vec<u32> = folds.iter().map(|fold| fold.id).collect();
        let (Pairing::Both { rhs: after, .. } | Pairing::RightOnly { rhs: after }) = &file.file
        else {
            unreachable!("a file with after-side regions has an after path");
        };
        let language = file.language.as_deref();
        let texts = self
            .call(Params {
                path: &after.path,
                language,
                src: &rhs.text,
                folds,
            })
            .context(Failure::Hook)?;
        let mut by_id = BTreeMap::new();
        for (key, text) in texts {
            let id: u32 = key
                .parse()
                .ok()
                .filter(|id| selected.contains(id))
                .ok_or_else(|| anyhow!("answered for unknown fold {key:?}"))
                .context(Failure::Hook)?;
            by_id.insert(id, text);
        }
        walk_mut(&mut rhs.regions, &mut |region| {
            if let Some(text) = by_id.get(&region.alignment_id) {
                collapse(region, summary_label(language, text));
            }
        });
        Ok(())
    }
}

/// Reserve a loopback port for the hook. The listener is released before the
/// hook starts, which is the usual small race on a single machine.
fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

/// Poll until the hook accepts connections, or fail early if it exits.
fn await_listening(child: &mut Child, port: u16, startup_timeout_ms: u64) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_millis(startup_timeout_ms);
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    loop {
        if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(anyhow!("fold hook exited during startup with {status}"));
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
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
    use crate::mutate::summarize::tests::project;
    use std::sync::Arc;

    fn hook(mode: &str, timeout_ms: u64) -> anyhow::Result<Hook> {
        let config = HookConfig {
            dir: env!("CARGO_MANIFEST_DIR").into(),
            command: vec![
                "python3".into(),
                "tests/hooks/rpc_server.py".into(),
                mode.into(),
            ],
            tags: Some(vec!["body".into()]),
            min_lines: Some(2),
            timeout_ms,
            startup_timeout_ms: 10_000,
        };
        Hook::spawn(&config, 12, Path::new("."))
    }

    const LARGE: &str = "def f():\n    a()\n    b()\n    c()\n\ndef g():\n    d()\n";

    fn fold_labels(sides: &Pairing<Source>) -> Vec<(bool, String)> {
        let mut labels = Vec::new();
        let (Pairing::Both { rhs: rhs_side, .. } | Pairing::RightOnly { rhs: rhs_side }) = &sides
        else {
            panic!("an after side");
        };
        walk(&rhs_side.regions, &mut |region| {
            if is_fold(region) {
                labels.push((region.visibility.collapsed, region.visibility.label.clone()));
            }
        });
        labels
    }

    #[test]
    fn summaries_land_on_selected_folds_only() {
        let hook = hook("echo", 5000).unwrap();
        let (file, mut sides) = project("file.py", "", LARGE);
        hook.apply(&file, &mut sides).unwrap();
        // `g` has a one-line body, which is not a region.
        assert_eq!(
            fold_labels(&sides),
            vec![(
                true,
                "# pseudocode
pseudo Body"
                    .to_owned()
            )]
        );
    }

    #[test]
    fn concurrent_calls_share_one_client() {
        let hook = Arc::new(hook("echo", 5000).unwrap());
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let hook = Arc::clone(&hook);
                std::thread::spawn(move || {
                    let (file, mut sides) = project("file.py", "", LARGE);
                    hook.apply(&file, &mut sides).unwrap();
                    fold_labels(&sides)[0].1.clone()
                })
            })
            .collect();
        for worker in workers {
            assert_eq!(worker.join().unwrap(), "# pseudocode\npseudo Body");
        }
    }

    #[test]
    fn small_paired_or_untagged_folds_never_reach_the_hook() {
        let hook = hook("error", 5000).unwrap();
        let (file, mut sides) = project("file.py", "", "import os\nimport sys\n");
        hook.apply(&file, &mut sides).unwrap();
        let (file, mut sides) = project("file.py", "", "def f():\n    a()\n");
        hook.apply(&file, &mut sides).unwrap();
        let (file, mut sides) = project("file.py", LARGE, LARGE);
        hook.apply(&file, &mut sides).unwrap();
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
    fn call_failures_are_reported_without_changing_the_regions() {
        let (file, mut sides) = project("file.py", "", LARGE);
        let error = hook("slow", 200)
            .unwrap()
            .apply(&file, &mut sides)
            .unwrap_err();
        assert!(format!("{error:#}").contains("timed out"), "{error:#}");
        let error = hook("error", 5000)
            .unwrap()
            .apply(&file, &mut sides)
            .unwrap_err();
        assert!(format!("{error:#}").contains("declined"), "{error:#}");
        let error = hook("bad", 5000)
            .unwrap()
            .apply(&file, &mut sides)
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("invalid response"),
            "{error:#}"
        );
        assert_eq!(error.downcast_ref::<Failure>(), Some(&Failure::Hook));
        assert!(fold_labels(&sides).iter().all(|(collapsed, _)| !collapsed));
    }
}
