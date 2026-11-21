use std::fmt;
use std::marker::PhantomData;

use rustc_hash::FxHashMap;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub fn serialize<S, V>(map: &FxHashMap<String, V>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    V: Serialize,
{
    let mut ser = serializer.serialize_map(Some(map.len()))?;
    for (k, v) in map {
        ser.serialize_entry(k, v)?;
    }
    ser.end()
}

pub fn deserialize<'de, D, V>(deserializer: D) -> Result<FxHashMap<String, V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct FxMapVisitor<V>(PhantomData<V>);

    impl<'de, V: Deserialize<'de>> Visitor<'de> for FxMapVisitor<V> {
        type Value = FxHashMap<String, V>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a map with string keys")
        }

        fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut map = FxHashMap::with_capacity_and_hasher(
                access.size_hint().unwrap_or(0),
                Default::default(),
            );
            while let Some((k, v)) = access.next_entry()? {
                map.insert(k, v);
            }
            Ok(map)
        }
    }

    deserializer.deserialize_map(FxMapVisitor(PhantomData))
}
