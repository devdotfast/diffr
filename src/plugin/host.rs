//! diffr's side of the host functions every plugin calls: `git` and `log`.
//! A component reaches them through its imports ([`super::wasm`]); a native
//! plugin through the SDK's host functions, which call [`Host`] itself
//! ([`super::native`]). Both run this code.
use anyhow::Context as _;
use diffr_plugin_sdk::host::Host as SdkHost;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

/// What one call of a plugin reads and runs things in.
#[derive(Clone)]
pub(crate) struct Host {
    pub(crate) name: Arc<str>,
    pub(crate) workdir: Arc<Path>,
}

impl Host {
    /// `git`'s stdout when it exits successfully, its stderr otherwise.
    /// `Err` is the host failing: git could not be started, or wrote
    /// something that is not UTF-8.
    pub(crate) fn git(&self, args: &[String]) -> anyhow::Result<Result<String, String>> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&*self.workdir)
            .output()
            .with_context(|| format!("plugin {}: running git {args:?}", self.name))?;
        let text = |bytes: Vec<u8>| {
            String::from_utf8(bytes)
                .with_context(|| format!("plugin {}: git {args:?} wrote non-UTF-8", self.name))
        };
        Ok(match output.status.success() {
            true => Ok(text(output.stdout)?),
            false => Err(text(output.stderr)?),
        })
    }

    pub(crate) fn log(&self, message: &str) {
        eprintln!("diffr plugin {}: {message}", self.name);
    }
}

impl SdkHost for Host {
    /// A failure of the host itself (git cannot be started, or wrote
    /// something that is not UTF-8) reaches the plugin as the call's error,
    /// exactly as it does in a component.
    fn git(&self, args: &[String]) -> Result<String, String> {
        Host::git(self, args).unwrap_or_else(|error| Err(format!("{error:#}")))
    }

    fn log(&self, message: &str) {
        Host::log(self, message)
    }
}
