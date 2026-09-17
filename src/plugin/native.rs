//! The native registry: the bundled plugins' code, compiled into diffr. Each
//! is an SDK [`Plugin`], the same trait a component's source implements, made
//! the same way from the same options string, and diffr calls it with the
//! same records it hands a component.
use super::host::Host;
use super::Runner;
use diffr_plugin_sdk::native as sdk;
use diffr_plugin_sdk::types::{FileEntry, Move, SourceSides};
#[cfg(test)]
use diffr_plugin_sdk::Plugin;
use std::rc::Rc;

// Generated registrations, discovered from native package metadata.
include!(concat!(env!("OUT_DIR"), "/native_plugins.rs"));

#[cfg(test)]
pub(crate) type Constructor = fn(Host, &str) -> anyhow::Result<Box<dyn Runner>>;

pub(crate) fn lookup(name: &str) -> anyhow::Result<Option<&'static sdk::Registration>> {
    lookup_in(PLUGINS, name)
}

fn lookup_in<'a>(
    plugins: &[&'a sdk::Registration],
    name: &str,
) -> anyhow::Result<Option<&'a sdk::Registration>> {
    let mut found = None;
    for &registration in plugins {
        if registration.name == name {
            anyhow::ensure!(
                found.is_none(),
                "duplicate native plugin registration: {name}"
            );
            found = Some(registration);
        }
    }
    Ok(found)
}

pub(crate) fn registered(
    registration: &sdk::Registration,
    host: Host,
    options: &str,
) -> anyhow::Result<Box<dyn Runner>> {
    let plugin = call(host, || (registration.create)(options))?;
    Ok(Box::new(Native(plugin)))
}

#[cfg(test)]
pub(crate) fn native<P: Plugin + Send + Sync + 'static>(
    host: Host,
    options: &str,
) -> anyhow::Result<Box<dyn Runner>> {
    let plugin = call(host, || sdk::create::<P>(options))?;
    Ok(Box::new(Native(plugin)))
}

fn call<R>(host: Host, call: impl FnOnce() -> anyhow::Result<R>) -> anyhow::Result<R> {
    diffr_plugin_sdk::host::scope(Rc::new(host), call)
}

struct Native(Box<dyn sdk::Instance>);

impl Runner for Native {
    fn queries(&self, host: Host) -> anyhow::Result<Vec<diffr_plugin_sdk::QuerySource>> {
        call(host, || self.0.queries())
    }
    fn classify(&self, host: Host, file: &FileEntry) -> anyhow::Result<Vec<String>> {
        call(host, || self.0.classify(file))
    }
    fn mutate(
        &self,
        host: Host,
        file: &FileEntry,
        sides: &SourceSides,
    ) -> anyhow::Result<Vec<Move>> {
        call(host, || self.0.mutate(file, sides))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unused(_: &str) -> anyhow::Result<Box<dyn sdk::Instance>> {
        anyhow::bail!("not instantiated")
    }

    #[test]
    fn lookup_rejects_duplicate_names_without_constructing_plugins() {
        let first = sdk::Registration {
            name: "example",
            create: unused,
        };
        let second = sdk::Registration {
            name: "example",
            create: unused,
        };
        assert!(std::ptr::eq(
            lookup_in(&[&first], "example").unwrap().unwrap(),
            &first
        ));
        assert!(lookup_in(&[&first], "absent").unwrap().is_none());
        assert!(lookup_in(&[&first, &second], "example")
            .err()
            .unwrap()
            .to_string()
            .contains("duplicate native plugin"));
    }
}
