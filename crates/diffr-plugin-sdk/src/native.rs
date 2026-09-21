//! Native registrations contributed by linked plugin crates.
use crate::types::SourceSides;
use crate::{tree, FileEntry, Move, Plugin, QuerySource};

/// The object-safe form of a plugin, after its options have been deserialized.
pub trait Instance: Send + Sync {
    fn queries(&self) -> anyhow::Result<Vec<QuerySource>>;
    fn enrich(
        &self,
        file: &FileEntry,
        sides: &SourceSides,
    ) -> anyhow::Result<Vec<crate::Annotation>>;
    fn classify(&self, file: &FileEntry) -> anyhow::Result<Vec<String>>;
    fn mutate(&self, file: &FileEntry, sides: &SourceSides) -> anyhow::Result<Vec<Move>>;
}

/// A name and constructor, registered without instantiating the plugin.
pub struct Registration {
    pub name: &'static str,
    pub create: fn(&str) -> anyhow::Result<Box<dyn Instance>>,
}

#[doc(hidden)]
pub fn create<P: Plugin + Send + Sync + 'static>(
    options: &str,
) -> anyhow::Result<Box<dyn Instance>> {
    let options = serde_json::from_str(options)
        .map_err(|error| anyhow::anyhow!("invalid options: {error}"))?;
    Ok(Box::new(Adapter(P::new(options)?)))
}

struct Adapter<P>(P);

impl<P: Plugin + Send + Sync> Instance for Adapter<P> {
    fn enrich(
        &self,
        file: &FileEntry,
        sides: &SourceSides,
    ) -> anyhow::Result<Vec<crate::Annotation>> {
        self.0.enrich(file, &tree::sides(sides)?)
    }
    fn queries(&self) -> anyhow::Result<Vec<QuerySource>> {
        self.0.queries()
    }
    fn classify(&self, file: &FileEntry) -> anyhow::Result<Vec<String>> {
        self.0.classify(file)
    }
    fn mutate(&self, file: &FileEntry, sides: &SourceSides) -> anyhow::Result<Vec<Move>> {
        self.0.mutate(file, &tree::sides(sides)?)
    }
}
