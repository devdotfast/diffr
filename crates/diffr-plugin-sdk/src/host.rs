//! What diffr gives every plugin: `git`, the `host` interface of
//! `wit/plugin.wit`. A plugin calls it the same way wherever it runs. In a
//! component it calls the import. Natively, diffr runs each call of a plugin
//! inside [`scope`], with its own implementation of [`Host`], and the
//! function calls that.
//!
//! Everything else a plugin needs it does itself: it reads files, and writes
//! to stderr, which diffr captures and writes to its own.

/// Run `git` with `args` in the repository's working directory: its stdout
/// when it exits successfully, its stderr otherwise.
pub fn git(args: &[String]) -> Result<String, String> {
    imp::git(args)
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use crate::bindings::diffr::plugin::host;

    pub(super) fn git(args: &[String]) -> Result<String, String> {
        host::git(args)
    }
}

/// diffr's implementation of the host functions for one call of a native
/// plugin.
#[cfg(not(target_arch = "wasm32"))]
pub trait Host {
    fn git(&self, args: &[String]) -> Result<String, String>;
}

/// Run `call`, one call of a native plugin, with `host` behind the host
/// function on this thread.
#[cfg(not(target_arch = "wasm32"))]
pub fn scope<R>(host: std::rc::Rc<dyn Host>, call: impl FnOnce() -> R) -> R {
    imp::scope(host, call)
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::Host;
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
                .expect("the host function is called only while diffr runs a plugin")
        })
    }

    pub(super) fn git(args: &[String]) -> Result<String, String> {
        current().git(args)
    }
}
