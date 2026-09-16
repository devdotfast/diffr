//! What diffr gives every plugin: `read_head`, `git` and `log`, the `host`
//! interface of `wit/plugin.wit`. A plugin calls these functions the same
//! way wherever it runs. In a component they call the imports. Natively,
//! diffr runs each call of a plugin inside [`scope`], with its own
//! implementation of [`Host`], and the functions call that.
use crate::types::Side;

/// Up to `max_bytes` from the start of one side of the file: during
/// `classify` the blob (or working-tree file), none for a side the file does
/// not have or one that is not a regular file; during `mutate` the side's
/// text as diffed; during `new` none.
pub fn read_head(side: Side, max_bytes: u32) -> Option<Vec<u8>> {
    imp::read_head(side, max_bytes)
}

/// Run `git` with `args` in the repository's working directory: its stdout
/// when it exits successfully, its stderr otherwise.
pub fn git(args: &[String]) -> Result<String, String> {
    imp::git(args)
}

/// Write a line to diffr's stderr, prefixed with the plugin's name.
pub fn log(message: &str) {
    imp::log(message)
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use crate::bindings::diffr::plugin::{host, types};
    use crate::types::Side;

    pub(super) fn read_head(side: Side, max_bytes: u32) -> Option<Vec<u8>> {
        let side = match side {
            Side::Lhs => types::Side::Lhs,
            Side::Rhs => types::Side::Rhs,
        };
        host::read_head(side, max_bytes)
    }

    pub(super) fn git(args: &[String]) -> Result<String, String> {
        host::git(args)
    }

    pub(super) fn log(message: &str) {
        host::log(message)
    }
}

/// diffr's implementation of the host functions for one call of a native
/// plugin. A failure of the host itself (git cannot be started, a blob
/// cannot be read) is diffr's to report: it fails the call once the plugin
/// returns, as a trap fails a component's call.
#[cfg(not(target_arch = "wasm32"))]
pub trait Host {
    fn read_head(&self, side: Side, max_bytes: u32) -> Option<Vec<u8>>;
    fn git(&self, args: &[String]) -> Result<String, String>;
    fn log(&self, message: &str);
}

/// Run `call`, one call of a native plugin, with `host` behind the host
/// functions on this thread.
#[cfg(not(target_arch = "wasm32"))]
pub fn scope<R>(host: std::rc::Rc<dyn Host>, call: impl FnOnce() -> R) -> R {
    imp::scope(host, call)
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::Host;
    use crate::types::Side;
    use std::cell::RefCell;
    use std::rc::Rc;

    thread_local! {
        static CURRENT: RefCell<Option<Rc<dyn Host>>> = const { RefCell::new(None) };
    }

    /// Restores the host of the enclosing scope, if any, when a scope ends.
    struct Restore(Option<Rc<dyn Host>>);

    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|current| *current.borrow_mut() = self.0.take());
        }
    }

    pub(super) fn scope<R>(host: Rc<dyn Host>, call: impl FnOnce() -> R) -> R {
        let _restore = Restore(CURRENT.with(|current| current.borrow_mut().replace(host)));
        call()
    }

    fn current() -> Rc<dyn Host> {
        CURRENT.with(|current| {
            current
                .borrow()
                .clone()
                .expect("a host function is called only while diffr runs a plugin")
        })
    }

    pub(super) fn read_head(side: Side, max_bytes: u32) -> Option<Vec<u8>> {
        current().read_head(side, max_bytes)
    }

    pub(super) fn git(args: &[String]) -> Result<String, String> {
        current().git(args)
    }

    pub(super) fn log(message: &str) {
        current().log(message)
    }
}
