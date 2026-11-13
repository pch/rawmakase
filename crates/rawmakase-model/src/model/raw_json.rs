//! A JSON object whose values are kept as the text they were written as, for
//! rewriting stored recipes without touching what is not rewritten.
//!
//! Reading JSON into `serde_json::Value` and writing it again is not byte-exact:
//! serde_json's default number parser can change a value's last digit, and a
//! recipe's `f32` settings would be written as their widened `f64` text. A
//! [`RawObject`] keeps each value's text, so only the keys a caller replaces or
//! drops change.
use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::value::RawValue;

/// A JSON object as an ordered list of keys and the text of their values.
#[derive(Clone, Debug, Default)]
pub struct RawObject(Vec<(String, Box<RawValue>)>);

impl RawObject {
    /// The object `text` holds; an error when it is not one. A key given twice
    /// keeps its last value, as reading it as a recipe does.
    pub fn parse(text: &str) -> serde_json::Result<Self> {
        serde_json::from_str(text)
    }
    /// The text of `key`'s value.
    pub fn get(&self, key: &str) -> Option<&RawValue> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_ref())
    }
    /// Sets `key` to `value`, where it is, or last.
    pub fn set(&mut self, key: &str, value: Box<RawValue>) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => self.0.push((key.to_string(), value)),
        }
    }
    /// Sets `key` to `value`, where it is, else right after `after` (last without it).
    pub fn set_after(&mut self, after: &str, key: &str, value: Box<RawValue>) {
        if self.get(key).is_some() {
            return self.set(key, value);
        }
        let at = self
            .0
            .iter()
            .position(|(k, _)| k == after)
            .map_or(self.0.len(), |i| i + 1);
        self.0.insert(at, (key.to_string(), value));
    }
    /// Keeps the keys `keep` says to; whether any went.
    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) -> bool {
        let before = self.0.len();
        self.0.retain(|(k, _)| keep(k));
        self.0.len() != before
    }
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &RawValue)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_ref()))
    }
    /// The object as JSON text: each value as it was read or set.
    pub fn to_text(&self) -> String {
        let mut out = String::from("{");
        for (i, (key, value)) in self.0.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&serde_json::to_string(key).expect("a key serializes"));
            out.push(':');
            out.push_str(value.get());
        }
        out.push('}');
        out
    }
    /// The object as a value of another object.
    pub fn to_raw(&self) -> Box<RawValue> {
        raw(self.to_text())
    }
}

/// `text`, which is JSON, as a raw value.
pub fn raw(text: String) -> Box<RawValue> {
    RawValue::from_string(text).expect("JSON written by serde_json")
}

/// `value` as a raw value.
pub fn raw_of(value: &impl Serialize) -> Box<RawValue> {
    raw(serde_json::to_string(value).expect("a value serializes"))
}

impl<'de> Deserialize<'de> for RawObject {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = RawObject;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<RawObject, A::Error> {
                let mut object = RawObject::default();
                while let Some(key) = map.next_key::<String>()? {
                    let value: Box<RawValue> = map.next_value()?;
                    object.set(&key, value);
                }
                Ok(object)
            }
        }
        d.deserialize_map(Object)
    }
}

impl Serialize for RawObject {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values not set keep their text to the byte, numbers included.
    #[test]
    fn values_keep_their_text() {
        let text = r#"{"a":0.10999999940395355,"b":[1, 2],"c":{"d":0.4},"e":"x"}"#;
        let mut o = RawObject::parse(text).unwrap();
        assert_eq!(o.get("a").unwrap().get(), "0.10999999940395355");
        o.set("e", raw_of(&"y"));
        o.set_after("a", "z", raw("{}".into()));
        assert!(o.retain(|k| k != "c"));
        assert_eq!(
            o.to_text(),
            r#"{"a":0.10999999940395355,"z":{},"b":[1, 2],"e":"y"}"#
        );
        assert!(RawObject::parse("[1]").is_err());
        assert_eq!(serde_json::to_string(&o).unwrap(), o.to_text());
    }
}
