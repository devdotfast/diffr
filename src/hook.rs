//! Trusted fold-summary subprocess: spawned once per session, NDJSON both ways.
//!
//! Each request carries one file and its large novel folds on the after side.
//! The hook replies in any order, keyed by request id; callers block on their
//! own reply so files still stream out as each worker finishes.
use crate::config::HookConfig;
use crate::hash::DftHashMap;
use crate::parse::folds::FoldMatch;
use crate::review::wire;
use crate::summary::{DiffResult, FileContent, FileFormat};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Serialize)]
struct Request<'a> {
    id: u64,
    path: &'a str,
    language: Option<&'static str>,
    src: &'a str,
    folds: Vec<RequestFold<'a>>,
}

#[derive(Serialize)]
struct RequestFold<'a> {
    /// Index into the file's rhs folds.
    id: usize,
    range: Value,
    tags: &'a [String],
    placeholder: &'a str,
}

#[derive(Deserialize)]
struct Reply {
    id: u64,
    #[serde(default)]
    texts: BTreeMap<String, String>,
    #[serde(default)]
    error: Option<String>,
}

enum Outcome {
    Reply(Reply),
    /// The hook can no longer answer anything.
    Closed(String),
}

type Pending = Arc<Mutex<Result<DftHashMap<u64, SyncSender<Outcome>>, String>>>;

pub(crate) struct Hook {
    config: HookConfig,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    pending: Pending,
    next_id: AtomicU64,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl Hook {
    pub(crate) fn spawn(config: &HookConfig, workspace: &Path) -> crate::git::Result<Self> {
        let mut child = Command::new(&config.command[0])
            .args(&config.command[1..])
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("could not start fold hook {:?}: {error}", config.command))?;
        let stdin = child.stdin.take().expect("piped hook stdin");
        let stdout = child.stdout.take().expect("piped hook stdout");
        let pending: Pending = Arc::new(Mutex::new(Ok(DftHashMap::default())));
        let reader = std::thread::spawn({
            let pending = Arc::clone(&pending);
            move || dispatch(BufReader::new(stdout), &pending)
        });
        Ok(Self {
            config: config.clone(),
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending,
            next_id: AtomicU64::new(1),
            reader: Mutex::new(Some(reader)),
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
                Some(crate::parse::guess_language::language_name(*language))
            }
            _ => None,
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = Request {
            id,
            path: &diff.display_path,
            language,
            src,
            folds: selected
                .iter()
                .map(|&index| {
                    let fold = &diff.rhs_folds[index];
                    RequestFold {
                        id: index,
                        range: wire::range(&fold.range),
                        tags: &fold.tags,
                        placeholder: &fold.placeholder,
                    }
                })
                .collect(),
        };
        let mut line = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
        line.push(b'\n');
        let (sender, receiver) = sync_channel(1);
        {
            let mut pending = self.pending.lock().unwrap();
            match pending.as_mut() {
                Ok(pending) => pending.insert(id, sender),
                Err(reason) => return Err(reason.clone()),
            };
        }
        {
            let mut stdin = self.stdin.lock().unwrap();
            if let Err(error) = stdin.write_all(&line).and_then(|()| stdin.flush()) {
                self.forget(id);
                return Err(format!("fold hook stdin closed: {error}"));
            }
        }
        let reply = match receiver.recv_timeout(Duration::from_millis(self.config.timeout_ms)) {
            Ok(Outcome::Reply(reply)) => reply,
            Ok(Outcome::Closed(reason)) => return Err(reason),
            Err(RecvTimeoutError::Timeout) => {
                self.forget(id);
                return Err(format!(
                    "fold hook timed out after {}ms",
                    self.config.timeout_ms
                ));
            }
            Err(RecvTimeoutError::Disconnected) => {
                unreachable!("reader drops senders only after signalling")
            }
        };
        if let Some(error) = reply.error {
            return Err(format!("fold hook reported: {error}"));
        }
        for (key, text) in reply.texts {
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

    fn forget(&self, id: u64) {
        if let Ok(pending) = self.pending.lock().unwrap().as_mut() {
            pending.remove(&id);
        }
    }
}

/// Route replies to their waiting request; a malformed line or EOF fails every
/// current and future request, since ids can no longer be trusted.
fn dispatch(stdout: BufReader<impl std::io::Read>, pending: &Pending) {
    let close = |reason: String| {
        let mut pending = pending.lock().unwrap();
        if let Ok(waiting) = std::mem::replace(&mut *pending, Err(reason.clone())) {
            for (_, sender) in waiting {
                let _ = sender.send(Outcome::Closed(reason.clone()));
            }
        }
    };
    for line in stdout.lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => return close(format!("fold hook stdout unreadable: {error}")),
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply: Reply = match serde_json::from_str(&line) {
            Ok(reply) => reply,
            Err(error) => return close(format!("fold hook wrote an invalid reply: {error}")),
        };
        let sender = match pending.lock().unwrap().as_mut() {
            Ok(waiting) => waiting.remove(&reply.id),
            Err(_) => return,
        };
        // A request that already timed out has no receiver; drop the late reply.
        if let Some(sender) = sender {
            let _ = sender.send(Outcome::Reply(reply));
        }
    }
    close("fold hook exited".into());
}

impl Drop for Hook {
    fn drop(&mut self) {
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
        if let Some(reader) = self.reader.lock().unwrap().take() {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn hook(script: &str, timeout_ms: u64) -> Hook {
        let config = HookConfig {
            command: vec!["sh".into(), "-c".into(), script.into()],
            tags: Some(vec!["body".into()]),
            min_lines: 2,
            timeout_ms,
        };
        Hook::spawn(&config, Path::new(".")).unwrap()
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
    /// Echo each request id back with a summary for fold 0.
    const ECHO: &str = r#"while IFS= read -r line; do id=$(printf '%s' "$line" | sed -E 's/^\{"id":([0-9]+).*/\1/'); printf '{"id":%s,"texts":{"0":"summary of f"}}\n' "$id"; done"#;

    #[test]
    fn summaries_land_on_selected_folds_only() {
        let hook = hook(ECHO, 5000);
        let mut result = diff(LARGE);
        assert_eq!(result.rhs_folds.len(), 2);
        hook.summarize(&mut result).unwrap();
        assert_eq!(result.rhs_folds[0].summary.as_deref(), Some("summary of f"));
        assert_eq!(result.rhs_folds[1].summary, None);
        assert!(result.lhs_folds.iter().all(|fold| fold.summary.is_none()));
    }

    #[test]
    fn concurrent_requests_are_routed_by_id() {
        let hook = Arc::new(hook(ECHO, 5000));
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
            assert_eq!(worker.join().unwrap().as_deref(), Some("summary of f"));
        }
    }

    #[test]
    fn small_or_untagged_folds_never_reach_the_hook() {
        let hook = hook("exit 3", 5000);
        let mut result = diff("import os\nimport sys\n");
        hook.summarize(&mut result).unwrap();
        let mut result = diff("def f():\n    a()\n");
        hook.summarize(&mut result).unwrap();
    }

    #[test]
    fn failures_are_reported_without_losing_the_diff() {
        let mut result = diff(LARGE);
        let error = hook("sleep 30", 50).summarize(&mut result).unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        let error = hook("exit 0", 5000).summarize(&mut result).unwrap_err();
        assert!(error.contains("exited"), "{error}");
        let error = hook("echo not json", 5000)
            .summarize(&mut result)
            .unwrap_err();
        assert!(error.contains("invalid reply"), "{error}");
        let error = hook(
            r#"read -r line; echo '{"id":1,"error":"rate limited"}'; sleep 30"#,
            5000,
        )
        .summarize(&mut result)
        .unwrap_err();
        assert!(error.contains("rate limited"), "{error}");
        let error = hook(
            r#"read -r line; echo '{"id":1,"texts":{"7":"x"}}'; sleep 30"#,
            5000,
        )
        .summarize(&mut result)
        .unwrap_err();
        assert!(error.contains("unknown fold"), "{error}");
        assert!(result.rhs_folds.iter().all(|fold| fold.summary.is_none()));
    }
}
