//! diffr's side of the host function every plugin calls: `git`. A component
//! reaches it through its import; a native plugin through the SDK's host
//! function, which calls [`Host`] itself ([`super::native`]). Both run this
//! code. Running git is the caller's: the CLI spawns it, and a caller with
//! no repository passes [`NoGit`].
use anyhow::Context as _;
use diffr_plugin_sdk::host::Host as SdkHost;
use std::path::Path;
use std::sync::Arc;

/// Runs git for plugins.
pub trait Git: Send + Sync {
    /// `git`'s stdout when it exits successfully, its stderr otherwise.
    /// `Err` is the host failing: git could not be started, or wrote
    /// something that is not UTF-8.
    fn run(&self, workdir: &Path, args: &[String]) -> anyhow::Result<Result<String, String>>;
}

/// No git: every call is the host failing.
pub struct NoGit;

impl Git for NoGit {
    fn run(&self, _: &Path, _: &[String]) -> anyhow::Result<Result<String, String>> {
        anyhow::bail!("git is not available here")
    }
}

/// What one call of a plugin reads and runs things in.
#[derive(Clone)]
pub struct Host {
    pub name: Arc<str>,
    pub workdir: Arc<Path>,
    pub git: Arc<dyn Git>,
}

impl Host {
    /// [`Git::run`] in the workdir, with errors naming the plugin.
    pub fn git(&self, args: &[String]) -> anyhow::Result<Result<String, String>> {
        self.git
            .run(&self.workdir, args)
            .with_context(|| format!("plugin {}: running git {args:?}", self.name))
    }
}

impl SdkHost for Host {
    /// A failure of the host itself (git cannot be started, or wrote
    /// something that is not UTF-8) reaches the plugin as the call's error,
    /// exactly as it does in a component.
    fn git(&self, args: &[String]) -> Result<String, String> {
        Host::git(self, args).unwrap_or_else(|error| Err(format!("{error:#}")))
    }
}
