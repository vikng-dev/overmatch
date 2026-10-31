//! A table as authored in a data file, keyed by name: the rounds' and guns' tables and the tank
//! surface's visual library parse through it.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};

/// A table as authored, refusing a duplicated key at parse: serde's default map fill would keep
/// the last of two entries of one key with nobody told.
pub(crate) struct KeyedMap<T>(pub(crate) BTreeMap<String, T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for KeyedMap<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for MapVisitor<T> {
            type Value = KeyedMap<T>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map of key to entry")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut out = BTreeMap::new();
                while let Some((name, entry)) = access.next_entry::<String, T>()? {
                    if out.contains_key(&name) {
                        return Err(de::Error::custom(format!(
                            "{name:?} is declared twice — the table is read by this key, so a \
                             duplicate is two answers for one name"
                        )));
                    }
                    out.insert(name, entry);
                }
                Ok(KeyedMap(out))
            }
        }

        deserializer.deserialize_map(MapVisitor(std::marker::PhantomData))
    }
}
