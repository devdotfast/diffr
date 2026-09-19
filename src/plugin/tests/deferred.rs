use super::*;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

struct WatchingWriter {
    bytes: Vec<u8>,
    file_flushed: Arc<AtomicBool>,
}
impl Write for WatchingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if String::from_utf8_lossy(&self.bytes)
            .lines()
            .any(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["type"] == "file")
        {
            self.file_flushed.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

struct Deferred {
    file_flushed: Arc<AtomicBool>,
    fail: bool,
}
impl Runner for Deferred {
    fn queries(&self, _: Host) -> anyhow::Result<Vec<types::QuerySource>> {
        Ok(vec![])
    }
    fn classify(&self, _: Host, _: &types::FileEntry) -> anyhow::Result<Vec<String>> {
        Ok(vec![])
    }
    fn mutate(
        &self,
        _: Host,
        _: &types::FileEntry,
        _: &types::SourceSides,
    ) -> anyhow::Result<Vec<types::Move>> {
        Ok(vec![])
    }
    fn enrich(
        &self,
        _: Host,
        _: &types::FileEntry,
        sides: &types::SourceSides,
    ) -> anyhow::Result<Vec<types::Annotation>> {
        assert!(
            self.file_flushed.load(Ordering::SeqCst),
            "initial file must be flushed before slow enrichment starts"
        );
        if self.fail {
            anyhow::bail!("model unavailable");
        }
        let sides = tree::sides(sides)?;
        let id = sides.rhs().unwrap().regions[0].id;
        Ok(vec![types::Annotation {
            region_id: id,
            label: "do work".into(),
        }])
    }
}

#[test]
fn initial_file_is_flushed_before_enrichment_and_survives_failure() {
    for fail in [false, true] {
        let file_flushed = Arc::new(AtomicBool::new(false));
        let mut pipeline = Pipeline::default();
        pipeline
            .push("deferred", json!({}), &|_, _| {
                Ok(Box::new(Deferred {
                    file_flushed: file_flushed.clone(),
                    fail,
                }))
            })
            .unwrap();
        let params = Config::from_toml("").unwrap().compile().unwrap();
        let mut output = WatchingWriter {
            bytes: Vec::new(),
            file_flushed,
        };
        let ended = crate::protocol::stream::write_file(
            "a.rs",
            "b.rs",
            (0, 20),
            || {
                crate::summary::DiffResult::try_from_sources_with_params(
                    "a.rs",
                    "",
                    "fn f() { work(); }\n",
                    &params,
                )
            },
            &params,
            &pipeline,
            crate::protocol::stream::Options {
                syntax: false,
                updates: true,
            },
            &mut output,
        )
        .unwrap();
        assert_eq!(ended.failed, fail);
        assert!(!ended.aborted);
        let events: Vec<serde_json::Value> = String::from_utf8(output.bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            events
                .iter()
                .map(|event| event["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["start", "file", "annotations", "complete"]
        );
        assert_eq!(events[0]["version"], 4);
        assert!(
            events[1]["diff"]["structural_changes"]["head"]
                .as_array()
                .unwrap()
                .len()
                > 0
        );
        assert_eq!(events[2].get("error").is_some(), fail);
        assert_eq!(events[3]["succeeded"], 1);
        assert_eq!(events[3]["failed"], 0);
    }
}
