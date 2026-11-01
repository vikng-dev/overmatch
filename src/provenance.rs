//! One number, one job. A sourced value is authored as `(value, Kind("note"))`.
//!
//! `Measured`, `Derived` and `Unknown` label a fact about the world: the note names the source,
//! the inputs and derivation, or what is missing and what would settle it. `Unit` declares a game
//! unit, a scale that balance lives in, and says why this scale. `Taste` is a magnitude chosen for
//! look or feel, multiplying a physical scalar, never replacing it; its note names the effect
//! family. `Resolution` says how finely or stably a law is evaluated, never what it states, and
//! gives the stability or cost reason. `StandIn` names the law or fact the game does not have yet.
//!
//! `Fixture` labels inline test data and loads only under `cfg(test)`. Shipped-file boundaries
//! use [`shipped`] in every build, including tests. Missing provenance, a blank note
//! or a fixture in shipped data is refused at load; the caller supplies the file and the parser
//! locates the field. The loader keeps only the value; the data file holds its job and note.

use serde::de::{self, Deserialize, Deserializer, SeqAccess, Visitor};

/// Where a value comes from. Each variant carries its source or reason, which may not be blank.
#[derive(serde::Deserialize)]
enum Provenance {
    /// Read off a measurement or a published datum; names the source.
    Measured(String),
    /// Computed from other values; names the inputs and the derivation.
    Derived(String),
    /// A game unit: a declared scale that balance lives in; says why this scale.
    Unit(String),
    /// A magnitude chosen for look or feel, multiplying a physical scalar, never replacing it;
    /// names the effect family it belongs to.
    Taste(String),
    /// A value standing in for a law or fact the game does not have yet; names that law or fact.
    StandIn(String),
    /// Test fixture data; accepted only under `cfg(test)`.
    Fixture(String),
    /// How finely or stably a law is evaluated, never what it states; gives the stability or cost reason.
    Resolution(String),
    /// Not known; says what is missing and what would settle it.
    Unknown(String),
}

impl Provenance {
    fn note(&self) -> &str {
        match self {
            Self::Measured(note)
            | Self::Derived(note)
            | Self::Unit(note)
            | Self::Taste(note)
            | Self::StandIn(note)
            | Self::Fixture(note)
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
        #[cfg(test)]
        if !FIXTURES_ALLOWED.get() {
            return deserialize_sourced::<T, D, false>(deserializer);
        }
        deserialize_sourced::<T, D, { cfg!(test) }>(deserializer)
    }
}

// Both admission policies use the same visitor and refusal.
fn deserialize_sourced<'de, T: Deserialize<'de>, D: Deserializer<'de>, const FIXTURES: bool>(
    deserializer: D,
) -> Result<Sourced<T>, D::Error> {
    struct SourcedVisitor<T, const FIXTURES: bool>(std::marker::PhantomData<T>);

    impl<'de, T: Deserialize<'de>, const FIXTURES: bool> Visitor<'de> for SourcedVisitor<T, FIXTURES> {
        type Value = Sourced<T>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(
                "a value and its provenance, `(value, Measured|Derived|Unknown|Unit|Taste|\
                 Resolution|StandIn(\"source or reason\"))` — a value without provenance does not load",
            )
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let value: T = seq
                .next_element()?
                .ok_or_else(|| de::Error::invalid_length(0, &self))?;
            let provenance: Provenance = seq.next_element()?.ok_or_else(|| {
                de::Error::custom("the value has no provenance — a value without one does not load")
            })?;
            if matches!(provenance, Provenance::Fixture(_)) && !FIXTURES {
                return Err(de::Error::custom(
                    "Fixture provenance is test-only — replace this field's label with its \
                     fact, Unit, Taste, Resolution or StandIn job and a non-blank note",
                ));
            }
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
    deserializer.deserialize_any(SourcedVisitor::<T, FIXTURES>(std::marker::PhantomData))
}

// Only inline tests admit fixtures. A shipped-file boundary temporarily refuses them on this
// test thread, including nested deserializers, and restores the caller's policy on return or panic.
#[cfg(test)]
thread_local! {
    static FIXTURES_ALLOWED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

#[cfg(test)]
struct ShippedPolicy(bool);

#[cfg(test)]
impl Drop for ShippedPolicy {
    fn drop(&mut self) {
        FIXTURES_ALLOWED.set(self.0);
    }
}

/// Load a shipped file under the refusing policy, naming its file in the actual admission error.
/// Ordinary builds already refuse fixtures; the scoped policy makes CI's test build refuse too.
pub(crate) fn shipped<T, E: std::fmt::Display>(
    file: &str,
    load: impl FnOnce() -> Result<T, E>,
) -> Result<T, String> {
    with_shipped_policy(|| load().map_err(|err| format!("{file}: {err}")))
}

/// Scope a synchronous shipped reader whose refusal already names its file, including panic gates.
pub(crate) fn with_shipped_policy<T>(load: impl FnOnce() -> T) -> T {
    #[cfg(test)]
    let _policy = ShippedPolicy(FIXTURES_ALLOWED.replace(false));
    load()
}

/// Deserialize a shipped text with the same policy and filename handling as other file loaders.
pub(crate) fn from_shipped_str<'de, T: Deserialize<'de>>(
    file: &str,
    text: &'de str,
) -> Result<T, String> {
    shipped(file, || ron::de::from_str(text))
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
        for kind in [
            "Measured",
            "Derived",
            "Unknown",
            "Unit",
            "Taste",
            "Resolution",
            "StandIn",
            "Fixture",
        ] {
            let probe = parse(&format!("(value: (2.5, {kind}(\"a reason\")))"))
                .unwrap_or_else(|err| panic!("{kind}: {err}"));
            assert_eq!(probe.value, Sourced(2.5));
        }
    }

    /// Shipped asset admission must refuse test data before play; tests alone accept it.
    #[test]
    fn fixtures_load_only_with_the_test_policy() {
        let text = "(\n    value: (2.5, Fixture(\"fixture\")),\n)";
        assert_eq!(parse(text).unwrap().value, Sourced(2.5));
        let report = from_shipped_str::<Probe>("fixture.ron", text).unwrap_err();
        assert!(report.contains("fixture.ron: 2:"), "{report}");
        assert!(
            report.contains("Fixture provenance is test-only"),
            "{report}"
        );
        assert!(report.contains("replace this field's label"), "{report}");
        let real: Probe = from_shipped_str(
            "fixture.ron",
            "(value: (2.5, Unit(\"declared pool scale\")))",
        )
        .unwrap();
        assert_eq!(real.value, Sourced(2.5));
        // A shipped refusal must not change the inline fixture policy on this test thread.
        assert_eq!(parse(text).unwrap().value, Sourced(2.5));
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
        for kind in [
            "Measured",
            "Derived",
            "Unknown",
            "Unit",
            "Taste",
            "Resolution",
            "StandIn",
            "Fixture",
        ] {
            for blank in ["", "   "] {
                let text = format!("(value: (2.5, {kind}(\"{blank}\")))");
                assert!(parse(&text).unwrap_err().contains("blank"), "{text}");
            }
        }
        for text in [
            "(value: (2.5, Tuned(\"a reason\")))",
            "(value: (2.5, Guessed(\"a reason\")))",
            "(value: (2.5, Measured(\"a\"), Derived(\"b\")))",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }
}
