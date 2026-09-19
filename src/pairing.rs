//! Which sides of a comparison a thing exists on.
//!
//! Sides are always `lhs` (before) and `rhs` (after). On the wire a
//! `Pairing` serializes by presence: `{lhs, rhs}`, `{lhs}`, or `{rhs}`.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Which sides a thing exists on. Serializes by presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pairing<T> {
    Both { lhs: T, rhs: T },
    LeftOnly { lhs: T },
    RightOnly { rhs: T },
}

impl<T> Pairing<T> {
    /// The same sides, each value transformed by `f`.
    pub(crate) fn map<U>(self, mut f: impl FnMut(T) -> U) -> Pairing<U> {
        match self {
            Self::Both { lhs, rhs } => Pairing::Both {
                lhs: f(lhs),
                rhs: f(rhs),
            },
            Self::LeftOnly { lhs } => Pairing::LeftOnly { lhs: f(lhs) },
            Self::RightOnly { rhs } => Pairing::RightOnly { rhs: f(rhs) },
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairingRepr<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    lhs: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rhs: Option<T>,
}

impl<T: Serialize> Serialize for Pairing<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let repr = match self {
            Self::Both { lhs, rhs } => PairingRepr {
                lhs: Some(lhs),
                rhs: Some(rhs),
            },
            Self::LeftOnly { lhs } => PairingRepr {
                lhs: Some(lhs),
                rhs: None,
            },
            Self::RightOnly { rhs } => PairingRepr {
                lhs: None,
                rhs: Some(rhs),
            },
        };
        repr.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Pairing<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match PairingRepr::deserialize(deserializer)? {
            PairingRepr {
                lhs: Some(lhs),
                rhs: Some(rhs),
            } => Ok(Self::Both { lhs, rhs }),
            PairingRepr {
                lhs: Some(lhs),
                rhs: None,
            } => Ok(Self::LeftOnly { lhs }),
            PairingRepr {
                lhs: None,
                rhs: Some(rhs),
            } => Ok(Self::RightOnly { rhs }),
            PairingRepr {
                lhs: None,
                rhs: None,
            } => Err(D::Error::custom("a pairing needs at least one side")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_pairing_needs_a_side() {
        let error = serde_json::from_value::<Pairing<u32>>(json!({})).unwrap_err();
        assert!(error.to_string().contains("at least one side"));
        assert_eq!(
            serde_json::from_value::<Pairing<u32>>(json!({"rhs": 7})).unwrap(),
            Pairing::RightOnly { rhs: 7 }
        );
    }
}

/// Original file identities and source trees, constrained together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Comparison<F, S> {
    Same {
        lhs_file: F,
        rhs_file: F,
        source: S,
    },
    Both {
        lhs_file: F,
        rhs_file: F,
        lhs: S,
        rhs: S,
    },
    LeftOnly {
        file: F,
        source: S,
    },
    RightOnly {
        file: F,
        source: S,
    },
}
impl<F, S> Comparison<F, S> {
    pub(crate) fn from_sides(files: Pairing<F>, sources: Pairing<S>) -> anyhow::Result<Self> {
        Ok(match (files, sources) {
            (
                Pairing::Both {
                    lhs: lhs_file,
                    rhs: rhs_file,
                },
                Pairing::Both { lhs, rhs },
            ) => Self::Both {
                lhs_file,
                rhs_file,
                lhs,
                rhs,
            },
            (Pairing::LeftOnly { lhs: file }, Pairing::LeftOnly { lhs: source }) => {
                Self::LeftOnly { file, source }
            }
            (Pairing::RightOnly { rhs: file }, Pairing::RightOnly { rhs: source }) => {
                Self::RightOnly { file, source }
            }
            _ => anyhow::bail!("file and source sides must agree"),
        })
    }
    pub(crate) fn files(&self) -> Pairing<&F> {
        match self {
            Self::Same {
                lhs_file, rhs_file, ..
            }
            | Self::Both {
                lhs_file, rhs_file, ..
            } => Pairing::Both {
                lhs: lhs_file,
                rhs: rhs_file,
            },
            Self::LeftOnly { file, .. } => Pairing::LeftOnly { lhs: file },
            Self::RightOnly { file, .. } => Pairing::RightOnly { rhs: file },
        }
    }
    pub(crate) fn source(&self, right: bool) -> Option<&S> {
        match self {
            Self::Same { source, .. } => Some(source),
            Self::Both { lhs, rhs, .. } => Some(if right { rhs } else { lhs }),
            Self::LeftOnly { source, .. } if !right => Some(source),
            Self::RightOnly { source, .. } if right => Some(source),
            _ => None,
        }
    }
    pub(crate) fn source_mut(&mut self, right: bool) -> Option<&mut S> {
        match self {
            Self::Same { source, .. } => Some(source),
            Self::Both { lhs, rhs, .. } => Some(if right { rhs } else { lhs }),
            Self::LeftOnly { source, .. } if !right => Some(source),
            Self::RightOnly { source, .. } if right => Some(source),
            _ => None,
        }
    }
    pub(crate) fn sources(&self) -> Vec<&S> {
        match self {
            Self::Both { lhs, rhs, .. } => vec![lhs, rhs],
            Self::Same { source, .. }
            | Self::LeftOnly { source, .. }
            | Self::RightOnly { source, .. } => vec![source],
        }
    }
    /// Visits a shared tree once.
    pub(crate) fn sources_mut(&mut self) -> Vec<&mut S> {
        match self {
            Self::Both { lhs, rhs, .. } => vec![lhs, rhs],
            Self::Same { source, .. }
            | Self::LeftOnly { source, .. }
            | Self::RightOnly { source, .. } => vec![source],
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum SourceWire<S> {
    Same { same: S },
    Sides(Pairing<S>),
}
#[derive(Serialize, Deserialize)]
struct ComparisonWire<F, S> {
    file: Pairing<F>,
    sources: SourceWire<S>,
}
impl<F: Serialize, S: Serialize> Serialize for Comparison<F, S> {
    fn serialize<W: Serializer>(&self, serializer: W) -> Result<W::Ok, W::Error> {
        let sources = match self {
            Self::Same { source, .. } => SourceWire::Same { same: source },
            Self::Both { lhs, rhs, .. } => SourceWire::Sides(Pairing::Both { lhs, rhs }),
            Self::LeftOnly { source, .. } => SourceWire::Sides(Pairing::LeftOnly { lhs: source }),
            Self::RightOnly { source, .. } => SourceWire::Sides(Pairing::RightOnly { rhs: source }),
        };
        ComparisonWire {
            file: self.files(),
            sources,
        }
        .serialize(serializer)
    }
}
impl<'de, F: Deserialize<'de>, S: Deserialize<'de>> Deserialize<'de> for Comparison<F, S> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ComparisonWire::<F, S>::deserialize(deserializer)?;
        match (wire.file, wire.sources) {
            (
                Pairing::Both {
                    lhs: lhs_file,
                    rhs: rhs_file,
                },
                SourceWire::Same { same: source },
            ) => Ok(Self::Same {
                lhs_file,
                rhs_file,
                source,
            }),
            (file, SourceWire::Sides(sources)) => {
                Self::from_sides(file, sources).map_err(D::Error::custom)
            }
            _ => Err(D::Error::custom(
                "shared source requires both file references",
            )),
        }
    }
}
