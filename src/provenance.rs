//! One number, one job. A sourced value is authored as `(value, Kind("note"))`.
//!
//! `Measured`, `Derived` and `Unknown` label a fact about the world: the note names the source,
//! the inputs and derivation, or what is missing and what would settle it. `Unit` declares a game
//! unit, a scale that balance lives in, and says why this scale. `Taste` is a magnitude chosen for
//! look or feel, multiplying a physical scalar, never replacing it; its note names the effect
//! family. `Resolution` says how finely or stably a law is evaluated, never what it states, and
//! gives the stability or cost reason. `StandIn` names the law or fact the game does not have yet.
//!
//! `Fixture` labels test data and loads only under `cfg(test)`. Missing provenance, a blank note
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
        deserialize_sourced::<T, D, { cfg!(test) }>(deserializer)
    }
}

// The test instantiates the shipped policy too, so it exercises the same visitor and refusal.
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
        #[derive(serde::Deserialize, Debug)]
        struct ShippedProbe {
            #[serde(deserialize_with = "deserialize_sourced::<f32, _, false>")]
            value: Sourced<f32>,
        }

        let text = "(\n    value: (2.5, Fixture(\"fixture\")),\n)";
        assert_eq!(parse(text).unwrap().value, Sourced(2.5));
        let err = ron::de::from_str::<ShippedProbe>(text).unwrap_err();
        assert_eq!(
            err.span.start.line, 2,
            "the parser locates the refused field"
        );
        let report = format!("fixture.ron: {err}");
        assert!(
            report.contains("fixture.ron") && report.contains("Fixture provenance is test-only")
        );
        assert!(report.contains("replace this field's label"));
        let real =
            ron::de::from_str::<ShippedProbe>("(value: (2.5, Unit(\"declared pool scale\")))")
                .unwrap();
        assert_eq!(real.value, Sourced(2.5));
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
