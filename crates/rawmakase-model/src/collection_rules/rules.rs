//! A smart collection's rules as a tree, with Lightroom's names: a [`Group`]
//! combines its [`Item`]s (rules and nested groups) as `union` (match any),
//! `intersect` (all) or `exclude` (none). A [`Rule`] keeps Lightroom's
//! `criteria`, `operation`, `value`, `value2` and `value_units` as written,
//! and any other key in `extra`, so a rule from Lightroom or a later release
//! is kept whole even where it isn't understood.
//!
//! Two text forms: Lightroom's Lua table ([`Group::from_lightroom`],
//! [`Group::to_lightroom`]) and RAWmakase's stored JSON ([`Group::to_json`],
//! [`Group::from_json`], version [`JSON_FORMAT`]), which keeps the same names
//! and unknown fields.
use super::lua::{self, Table, Value};
use std::collections::BTreeMap;

/// The version of [`Group::to_json`]'s form, stored beside it.
pub const JSON_FORMAT: i64 = 1;

/// How a group's items combine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combine {
    /// Match any (`"union"`).
    Union,
    /// Match all (`"intersect"`).
    Intersect,
    /// Match none (`"exclude"`).
    Exclude,
}
impl Combine {
    pub fn lightroom(self) -> &'static str {
        match self {
            Self::Union => "union",
            Self::Intersect => "intersect",
            Self::Exclude => "exclude",
        }
    }
    pub fn from_lightroom(s: &str) -> Option<Self> {
        [Self::Union, Self::Intersect, Self::Exclude]
            .into_iter()
            .find(|c| c.lightroom() == s)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub combine: Combine,
    pub items: Vec<Item>,
    /// Keys other than `combine`, kept as read.
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Rule(Rule),
    Group(Group),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub criteria: String,
    pub operation: String,
    pub value: Option<Value>,
    pub value2: Option<Value>,
    /// `value_units`: "days", "weeks", "months" or "years" for relative dates.
    pub units: Option<String>,
    /// Keys other than the five above, kept as read.
    pub extra: BTreeMap<String, Value>,
}
impl Rule {
    pub fn new(criteria: &str, operation: &str, value: Option<Value>) -> Self {
        Self {
            criteria: criteria.into(),
            operation: operation.into(),
            value,
            value2: None,
            units: None,
            extra: BTreeMap::new(),
        }
    }
}

/// Why rules were not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Malformed(pub String);
impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Malformed {}

const RULE_KEYS: [&str; 5] = ["criteria", "operation", "value", "value2", "value_units"];
/// The JSON form's own keys, which a Lightroom key may not take.
const GROUP_JSON_KEYS: [&str; 2] = ["combine", "items"];

impl Group {
    pub fn new(combine: Combine, items: Vec<Item>) -> Self {
        Self {
            combine,
            items,
            extra: BTreeMap::new(),
        }
    }
    /// Reads Lightroom's text (`AgLibraryCollectionContent.content` of an
    /// `ag.library.smart_collection` row).
    pub fn from_lightroom(text: &str) -> Result<Self, Malformed> {
        let table = lua::parse(text).map_err(|e| Malformed(e.to_string()))?;
        Self::from_table(&table)
    }
    fn from_table(table: &Table) -> Result<Self, Malformed> {
        let combine = match table.get("combine") {
            Some(Value::Str(s)) => Combine::from_lightroom(s)
                .ok_or_else(|| Malformed(format!("unknown combine \"{s}\"")))?,
            Some(_) => return Err(Malformed("combine is not a string".into())),
            None => return Err(Malformed("a group has no combine".into())),
        };
        let mut extra = BTreeMap::new();
        for (key, value) in &table.fields {
            if key == "combine" {
                continue;
            }
            if GROUP_JSON_KEYS.contains(&key.as_str()) {
                return Err(Malformed(format!("a group has a key \"{key}\"")));
            }
            extra.insert(key.clone(), value.clone());
        }
        let items = table
            .array
            .iter()
            .map(|v| match v {
                Value::Table(t) if t.get("criteria").is_some() => {
                    Rule::from_table(t).map(Item::Rule)
                }
                Value::Table(t) => Self::from_table(t).map(Item::Group),
                _ => Err(Malformed(
                    "a group holds something other than a table".into(),
                )),
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            combine,
            items,
            extra,
        })
    }
    /// The text Lightroom writes for these rules.
    pub fn to_lightroom(&self) -> String {
        lua::write(&self.to_table())
    }
    fn to_table(&self) -> Table {
        let mut fields: Vec<(String, Value)> = self
            .extra
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        fields.push((
            "combine".into(),
            Value::Str(self.combine.lightroom().into()),
        ));
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        Table {
            array: self
                .items
                .iter()
                .map(|item| {
                    Value::Table(match item {
                        Item::Rule(r) => r.to_table(),
                        Item::Group(g) => g.to_table(),
                    })
                })
                .collect(),
            fields,
        }
    }
    /// The stored JSON form: `{"combine": …, "items": […]}` with each rule
    /// an object of Lightroom's keys, and unknown keys beside them.
    pub fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (k, v) in &self.extra {
            map.insert(k.clone(), value_to_json(v));
        }
        map.insert("combine".into(), self.combine.lightroom().into());
        map.insert(
            "items".into(),
            self.items
                .iter()
                .map(|item| match item {
                    Item::Rule(r) => r.to_json(),
                    Item::Group(g) => g.to_json(),
                })
                .collect(),
        );
        serde_json::Value::Object(map)
    }
    pub fn from_json(json: &serde_json::Value) -> Result<Self, Malformed> {
        Self::from_json_at(json, 0)
    }
    fn from_json_at(json: &serde_json::Value, depth: usize) -> Result<Self, Malformed> {
        if depth > lua::MAX_DEPTH {
            return Err(Malformed("groups are nested too deeply".into()));
        }
        let Some(map) = json.as_object() else {
            return Err(Malformed("a group is not an object".into()));
        };
        let combine = map
            .get("combine")
            .and_then(|c| c.as_str())
            .and_then(Combine::from_lightroom)
            .ok_or_else(|| Malformed("a group has no known combine".into()))?;
        let items = map
            .get("items")
            .and_then(|i| i.as_array())
            .ok_or_else(|| Malformed("a group has no items".into()))?
            .iter()
            .map(|item| {
                if item.get("criteria").is_some() {
                    Rule::from_json(item).map(Item::Rule)
                } else {
                    Self::from_json_at(item, depth + 1).map(Item::Group)
                }
            })
            .collect::<Result<_, _>>()?;
        let mut extra = BTreeMap::new();
        for (k, v) in map {
            if !GROUP_JSON_KEYS.contains(&k.as_str()) {
                extra.insert(k.clone(), value_from_json(v, depth)?);
            }
        }
        Ok(Self {
            combine,
            items,
            extra,
        })
    }
    /// Every rule, nested ones too.
    pub fn rules(&self) -> Vec<&Rule> {
        let mut out = Vec::new();
        let mut stack = vec![self];
        while let Some(group) = stack.pop() {
            for item in &group.items {
                match item {
                    Item::Rule(r) => out.push(r),
                    Item::Group(g) => stack.push(g),
                }
            }
        }
        out
    }
}

impl Rule {
    fn from_table(table: &Table) -> Result<Self, Malformed> {
        let text = |key: &str| match table.get(key) {
            Some(Value::Str(s)) => Ok(Some(s.clone())),
            None => Ok(None),
            Some(_) => Err(Malformed(format!("a rule's {key} is not a string"))),
        };
        let criteria = text("criteria")?.unwrap_or_default();
        let operation = text("operation")?
            .ok_or_else(|| Malformed(format!("a {criteria} rule has no operation")))?;
        if !table.array.is_empty() {
            return Err(Malformed(format!("a {criteria} rule holds a list")));
        }
        Ok(Self {
            units: text("value_units")?,
            value: table.get("value").cloned(),
            value2: table.get("value2").cloned(),
            extra: table
                .fields
                .iter()
                .filter(|(k, _)| !RULE_KEYS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            criteria,
            operation,
        })
    }
    fn fields(&self) -> Vec<(String, Value)> {
        let mut fields: Vec<(String, Value)> = self
            .extra
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        fields.push(("criteria".into(), Value::Str(self.criteria.clone())));
        fields.push(("operation".into(), Value::Str(self.operation.clone())));
        for (key, value) in [("value", &self.value), ("value2", &self.value2)] {
            if let Some(v) = value {
                fields.push((key.into(), v.clone()));
            }
        }
        if let Some(units) = &self.units {
            fields.push(("value_units".into(), Value::Str(units.clone())));
        }
        // Lightroom writes a rule's keys in this order.
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        fields
    }
    fn to_table(&self) -> Table {
        Table {
            array: Vec::new(),
            fields: self.fields(),
        }
    }
    fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.fields()
                .iter()
                .map(|(k, v)| (k.clone(), value_to_json(v)))
                .collect(),
        )
    }
    fn from_json(json: &serde_json::Value) -> Result<Self, Malformed> {
        let Some(map) = json.as_object() else {
            return Err(Malformed("a rule is not an object".into()));
        };
        let mut table = Table::default();
        for (k, v) in map {
            table.fields.push((k.clone(), value_from_json(v, 0)?));
        }
        Self::from_table(&table)
    }
}

/// A Lua value in JSON: a table with only positional entries as an array,
/// otherwise as an object whose positional entries are keyed `"[1]"`,
/// `"[2]"`…, and whose named keys that start with `[` get one more `[`
/// in front, so neither can be taken for the other.
fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => {
            serde_json::Number::from_f64(*n).map_or(serde_json::Value::Null, Into::into)
        }
        Value::Str(s) => s.as_str().into(),
        Value::Table(t) if t.fields.is_empty() => t.array.iter().map(value_to_json).collect(),
        Value::Table(t) => {
            let mut map = serde_json::Map::new();
            for (i, v) in t.array.iter().enumerate() {
                map.insert(format!("[{}]", i + 1), value_to_json(v));
            }
            for (k, v) in &t.fields {
                let key = if k.starts_with('[') {
                    format!("[{k}")
                } else {
                    k.clone()
                };
                map.insert(key, value_to_json(v));
            }
            serde_json::Value::Object(map)
        }
    }
}

fn value_from_json(json: &serde_json::Value, depth: usize) -> Result<Value, Malformed> {
    if depth > lua::MAX_DEPTH {
        return Err(Malformed("values are nested too deeply".into()));
    }
    Ok(match json {
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => Value::Number(
            n.as_f64()
                .filter(|n| n.is_finite())
                .ok_or_else(|| Malformed("a number is out of range".into()))?,
        ),
        serde_json::Value::String(s) => Value::Str(s.clone()),
        serde_json::Value::Array(items) => Value::Table(Table {
            array: items
                .iter()
                .map(|v| value_from_json(v, depth + 1))
                .collect::<Result<_, _>>()?,
            fields: Vec::new(),
        }),
        serde_json::Value::Object(map) => {
            let mut table = Table::default();
            let mut positional = Vec::new();
            for (k, v) in map {
                let v = value_from_json(v, depth + 1)?;
                if let Some(named) = k.strip_prefix("[[") {
                    table.fields.push((format!("[{named}"), v));
                    continue;
                }
                match k
                    .strip_prefix('[')
                    .and_then(|k| k.strip_suffix(']'))
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    Some(n) => positional.push((n, v)),
                    None => table.fields.push((k.clone(), v)),
                }
            }
            positional.sort_by_key(|(n, _)| *n);
            table.array = positional.into_iter().map(|(_, v)| v).collect();
            Value::Table(table)
        }
        serde_json::Value::Null => return Err(Malformed("a value is null".into())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lightroom Classic 15.6's text for a nested group, as saved (pass 3).
    const NESTED: &str = "s = {\n\t{\n\t\tcriteria = \"rating\",\n\t\toperation = \">=\",\n\t\tvalue = 0,\n\t},\n\t{\n\t\t{\n\t\t\tcriteria = \"rating\",\n\t\t\toperation = \">=\",\n\t\t\tvalue = 0,\n\t\t},\n\t\t{\n\t\t\t{\n\t\t\t\tcriteria = \"rating\",\n\t\t\t\toperation = \">=\",\n\t\t\t\tvalue = 0,\n\t\t\t},\n\t\t\tcombine = \"intersect\",\n\t\t},\n\t\t{\n\t\t\t{\n\t\t\t\tcriteria = \"rating\",\n\t\t\t\toperation = \">=\",\n\t\t\t\tvalue = 0,\n\t\t\t},\n\t\t\tcombine = \"exclude\",\n\t\t},\n\t\tcombine = \"union\",\n\t},\n\tcombine = \"exclude\",\n}\n";
    /// Capture Date rules as Lightroom saved them, with stale `value2`s.
    const DATES: &str = "s = {\n\t{\n\t\tcriteria = \"captureTime\",\n\t\toperation = \"inLast\",\n\t\tvalue = 1,\n\t\tvalue2 = 1,\n\t\tvalue_units = \"days\",\n\t},\n\t{\n\t\tcriteria = \"captureTime\",\n\t\toperation = \"in\",\n\t\tvalue = \"2026-11-17\",\n\t\tvalue2 = \"2026-11-17\",\n\t},\n\t{\n\t\tcriteria = \"captureTime\",\n\t\toperation = \"thisWeek\",\n\t},\n\tcombine = \"intersect\",\n}\n";

    #[test]
    fn nested_groups_are_read_with_their_combines() {
        let group = Group::from_lightroom(NESTED).unwrap();
        assert_eq!(group.combine, Combine::Exclude);
        let Item::Group(inner) = &group.items[1] else {
            panic!("a nested group");
        };
        assert_eq!(inner.combine, Combine::Union);
        assert!(matches!(&inner.items[2], Item::Group(g) if g.combine == Combine::Exclude));
        assert_eq!(group.rules().len(), 4);
        assert_eq!(group.to_lightroom(), NESTED);
    }
    #[test]
    fn rules_round_trip_through_lightroom_text_and_json() {
        for text in [NESTED, DATES] {
            let group = Group::from_lightroom(text).unwrap();
            assert_eq!(group.to_lightroom(), text);
            let json = serde_json::to_string(&group.to_json()).unwrap();
            let back = Group::from_json(&serde_json::from_str(&json).unwrap()).unwrap();
            assert_eq!(back, group);
            assert_eq!(back.to_lightroom(), text);
        }
        let group = Group::from_lightroom(DATES).unwrap();
        let Item::Rule(rule) = &group.items[0] else {
            panic!("a rule");
        };
        assert_eq!(rule.units.as_deref(), Some("days"));
        assert_eq!(rule.value2, Some(Value::Number(1.)));
    }
    #[test]
    fn unknown_keys_are_kept_both_ways() {
        let text = "s = {\n\t{\n\t\tcriteria = \"rating\",\n\t\tfuture = {\n\t\t\t1,\n\t\t\tx = true,\n\t\t},\n\t\toperation = \"==\",\n\t\tvalue = 3,\n\t},\n\tcombine = \"union\",\n\tzeta = \"z\",\n}\n";
        let group = Group::from_lightroom(text).unwrap();
        assert_eq!(group.extra.get("zeta"), Some(&Value::Str("z".into())));
        assert_eq!(group.to_lightroom(), text);
        let json = group.to_json();
        assert_eq!(
            json["items"][0]["future"],
            serde_json::json!({"[1]": 1.0, "x": true})
        );
        assert_eq!(Group::from_json(&json).unwrap(), group);
        // A field a later release adds to the JSON form survives too.
        let mut later = json;
        later["items"][0]["note"] = serde_json::json!(["a", "b"]);
        let read = Group::from_json(&later).unwrap();
        assert_eq!(read.to_json(), later);
    }
    /// Review, round 1: a named key that looks like a position stays a
    /// named key through the JSON form.
    #[test]
    fn positional_and_bracketed_keys_survive_json() {
        for future in [
            "{ 42, [\"[1]\"] = \"named\", }",
            "{ [\"[1]\"] = \"named\", }",
            "{ [\"[[x\"] = 1, 7, 8, }",
        ] {
            let text = format!(
                "s = {{ {{ criteria = \"rating\", operation = \"==\", value = 3, future = {future} }}, combine = \"union\", }}"
            );
            let group = Group::from_lightroom(&text).unwrap();
            let json = serde_json::to_string(&group.to_json()).unwrap();
            let back = Group::from_json(&serde_json::from_str(&json).unwrap()).unwrap();
            assert_eq!(back, group, "{future} as {json}");
        }
    }
    #[test]
    fn malformed_rules_are_refused() {
        for text in [
            "s = { { criteria = \"rating\", operation = \"==\" } }",
            "s = { combine = \"sometimes\" }",
            "s = { 5, combine = \"union\" }",
            "s = { { criteria = 1, operation = \"==\" }, combine = \"union\" }",
            "s = { { criteria = \"rating\" }, combine = \"union\" }",
            "s = { items = 1, combine = \"union\" }",
        ] {
            assert!(Group::from_lightroom(text).is_err(), "{text}");
        }
    }
}
