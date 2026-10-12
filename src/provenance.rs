//! Provenance of an authored data value: where the number comes from, so a validation report can
//! show how far the data sits from truth.
//!
//! A sourced value is authored as `(value, Provenance("source or reason"))`, one of
//! `Measured`, `Derived`, `Tuned`, `Resolution` or `Unknown`. A value authored without one, or with
//! an empty source or reason, does not load. The loader checks the provenance and keeps the value;
//! the data file is where the provenance is read.

use serde::de::{self, Deserialize, Deserializer, SeqAccess, Visitor};

/// Where a value comes from. Each variant carries its source or reason, which may not be blank.
#[derive(serde::Deserialize)]
enum Provenance {
    /// Read off a measurement or a published datum; names the source.
    Measured(String),
    /// Computed from other values; names the inputs and the derivation.
    Derived(String),
    /// Chosen; gives the reason (a balance or modelling choice, not a fact).
    Tuned(String),
    /// A sampling or resolution choice: how finely a law is evaluated, never what it states.
    Resolution(String),
    /// Not known; says what is missing and what would settle it.
    Unknown(String),
}

impl Provenance {
    fn note(&self) -> &str {
        match self {
            Self::Measured(note)
            | Self::Derived(note)
            | Self::Tuned(note)
            | Self::Resolution(note)
            | Self::Unknown(note) => note,
        }
    }
}

/// A value whose provenance was present and non-blank at load.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sourced<T>(pub(crate) T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Sourced<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SourcedVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for SourcedVisitor<T> {
            type Value = Sourced<T>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(
                    "a value and its provenance, `(value, Measured|Derived|Tuned|Resolution|\
                     Unknown(\"source or reason\"))` — a value without provenance does not load",
                )
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let value: T = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let provenance: Provenance = seq.next_element()?.ok_or_else(|| {
                    de::Error::custom(
                        "the value has no provenance — a value without one does not load",
                    )
                })?;
                if provenance.note().trim().is_empty() {
                    return Err(de::Error::custom(
                        "the provenance names no source or reason — a blank one does not load",
                    ));
                }
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::invalid_length(3, &self));
                }
                Ok(Sourced(value))
            }
        }

        // `deserialize_any`, not `deserialize_tuple`: a bare value then reaches the visitor and is
        // refused with `expecting`'s message, which names the missing provenance.
        deserializer.deserialize_any(SourcedVisitor(std::marker::PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize, Debug)]
    #[serde(deny_unknown_fields)]
    struct Probe {
        value: Sourced<f32>,
    }

    fn parse(text: &str) -> Result<Probe, String> {
        ron::de::from_str(text).map_err(|err| err.to_string())
    }

    #[test]
    fn every_kind_of_provenance_loads_its_value() {
        for kind in ["Measured", "Derived", "Tuned", "Resolution", "Unknown"] {
            let probe = parse(&format!("(value: (2.5, {kind}(\"a reason\")))"))
                .unwrap_or_else(|err| panic!("{kind}: {err}"));
            assert_eq!(probe.value, Sourced(2.5));
        }
    }

    #[test]
    fn a_value_without_provenance_does_not_load() {
        for text in ["(value: 2.5)", "(value: (2.5))", "(value: (2.5,))"] {
            let err = parse(text).expect_err(text);
            assert!(err.contains("provenance"), "{text}: {err}");
        }
    }

    #[test]
    fn a_blank_or_unknown_provenance_does_not_load() {
        for text in [
            "(value: (2.5, Measured(\"\")))",
            "(value: (2.5, Tuned(\"   \")))",
            "(value: (2.5, Guessed(\"a reason\")))",
            "(value: (2.5, Measured(\"a\"), Derived(\"b\")))",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }
}
