//! What the smart collection editor offers: the fields RAWmakase evaluates,
//! in Lightroom Classic 15.6's menu sections and names, each with its
//! operations as Lightroom lists them and the value each takes. Values are
//! stored as Lightroom stores them (a label colour's number, a shutter
//! speed's APEX value), so a rule made here reads the same in Lightroom.
//! Fields RAWmakase can't evaluate are not offered.
use super::lua::Value;
use super::rules::Rule;

/// What an operation's value is entered as.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    /// Nothing: "is today", "is empty".
    None,
    Text,
    Number,
    /// Two values of `Range`'s kind, `value` and `value2`.
    Range(Single),
    /// 0 to 5 stars.
    Rating,
    /// Seconds, entered as "1/250" or "2", stored as APEX Tv = −log₂(t).
    Shutter,
    /// An f-number, stored as APEX Av = 2·log₂(N).
    Aperture,
    /// "YYYY-MM-DD".
    Date,
    /// A number of days, weeks, months or years (`value_units`).
    Period,
    /// One of a list.
    Choice(&'static [(&'static str, Choice)]),
}

/// What each end of a range is entered as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Single {
    Number,
    Rating,
    Shutter,
    Aperture,
    Date,
}
impl Single {
    /// The input one value of this kind takes.
    pub const fn input(self) -> Input {
        match self {
            Self::Number => Input::Number,
            Self::Rating => Input::Rating,
            Self::Shutter => Input::Shutter,
            Self::Aperture => Input::Aperture,
            Self::Date => Input::Date,
        }
    }
}

/// A listed value as Lightroom stores it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Choice {
    Number(f64),
    Text(&'static str),
    Bool(bool),
}
impl Choice {
    pub fn value(self) -> Value {
        match self {
            Self::Number(n) => Value::Number(n),
            Self::Text(t) => Value::Str(t.into()),
            Self::Bool(b) => Value::Bool(b),
        }
    }
}

/// One entry of an operation menu: Lightroom's stored operation, its label
/// and the value it takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Operation {
    pub op: &'static str,
    pub label: &'static str,
    pub input: Input,
}

/// One field of the editor's menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Field {
    pub criteria: &'static str,
    pub name: &'static str,
    /// The submenu it is in, as Lightroom's; None at the top.
    pub section: Option<&'static str>,
    pub operations: &'static [Operation],
}

const fn op(op: &'static str, label: &'static str, input: Input) -> Operation {
    Operation { op, label, input }
}

/// The units a period can be in.
pub const PERIOD_UNITS: [(&str, &str); 4] = [
    ("days", "days"),
    ("weeks", "weeks"),
    ("months", "months"),
    ("years", "years"),
];

const TEXT: &[Operation] = &[
    op("any", "contains", Input::Text),
    op("all", "contains all", Input::Text),
    op("words", "contains words", Input::Text),
    op("noneOf", "doesn’t contain", Input::Text),
    op("beginsWith", "starts with", Input::Text),
    op("endsWith", "ends with", Input::Text),
];
const TEXT_OR_EMPTY: &[Operation] = &[
    op("any", "contains", Input::Text),
    op("all", "contains all", Input::Text),
    op("words", "contains words", Input::Text),
    op("noneOf", "doesn’t contain", Input::Text),
    op("beginsWith", "starts with", Input::Text),
    op("endsWith", "ends with", Input::Text),
    op("empty", "is empty", Input::None),
    op("notEmpty", "isn’t empty", Input::None),
];
const KEYWORDS: &[Operation] = &[
    op("any", "contains", Input::Text),
    op("all", "contains all", Input::Text),
    op("words", "contains words", Input::Text),
    op("noneOf", "doesn’t contain", Input::Text),
    op("beginsWith", "starts with", Input::Text),
    op("endsWith", "ends with", Input::Text),
    op("empty", "are empty", Input::None),
    op("notEmpty", "aren’t empty", Input::None),
];
const IS_OR_TEXT: &[Operation] = &[
    op("==", "is", Input::Text),
    op("!=", "is not", Input::Text),
    op("any", "contains", Input::Text),
    op("all", "contains all", Input::Text),
    op("words", "contains words", Input::Text),
    op("noneOf", "doesn’t contain", Input::Text),
    op("beginsWith", "starts with", Input::Text),
    op("endsWith", "ends with", Input::Text),
];
const FILENAME: &[Operation] = &[
    op("any", "contains", Input::Text),
    op("all", "contains all", Input::Text),
    op("noneOf", "doesn’t contain", Input::Text),
    op("beginsWith", "starts with", Input::Text),
    op("endsWith", "ends with", Input::Text),
];
const COPY_NAME: &[Operation] = &[
    op("any", "contains", Input::Text),
    op("all", "contains all", Input::Text),
    op("noneOf", "doesn’t contain", Input::Text),
    op("beginsWith", "starts with", Input::Text),
    op("endsWith", "ends with", Input::Text),
    op("empty", "is empty", Input::None),
    op("notEmpty", "isn’t empty", Input::None),
];
const LABEL_TEXT: &[Operation] = &[
    op("==", "is", Input::Text),
    op("!=", "is not", Input::Text),
    op("empty", "is empty", Input::None),
    op("notEmpty", "isn’t empty", Input::None),
];
const fn numbers(kind: Single) -> [Operation; 7] {
    let input = kind.input();
    [
        op("==", "is", input),
        op("!=", "is not", input),
        op(">", "is greater than", input),
        op("<", "is less than", input),
        op(">=", "is greater than or equal to", input),
        op("<=", "is less than or equal to", input),
        op("in", "is in range", Input::Range(kind)),
    ]
}
const NUMBER: &[Operation] = &numbers(Single::Number);
const RATING: &[Operation] = &numbers(Single::Rating);
const SHUTTER: &[Operation] = &numbers(Single::Shutter);
const APERTURE: &[Operation] = &numbers(Single::Aperture);
const DATE: &[Operation] = &[
    op("==", "is", Input::Date),
    op("!=", "is not", Input::Date),
    op(">", "is after", Input::Date),
    op("<", "is before", Input::Date),
    op("inLast", "is in the last", Input::Period),
    op("notInLast", "is not in the last", Input::Period),
    op("in", "is in the range", Input::Range(Single::Date)),
    op("today", "is today", Input::None),
    op("yesterday", "is yesterday", Input::None),
    op("thisWeek", "is in this week", Input::None),
    op("thisMonth", "is in this month", Input::None),
    op("thisYear", "is in this year", Input::None),
];
const BOOL: &[Operation] = &[
    op("isTrue", "is true", Input::None),
    op("isFalse", "is false", Input::None),
];
const fn is_one_of(choices: &'static [(&'static str, Choice)]) -> [Operation; 2] {
    [
        op("==", "is", Input::Choice(choices)),
        op("!=", "is not", Input::Choice(choices)),
    ]
}
const PICK: &[Operation] = &is_one_of(&[
    ("flagged", Choice::Number(1.)),
    ("unflagged", Choice::Number(0.)),
    ("rejected", Choice::Number(-1.)),
]);
const LABEL_COLOR: &[Operation] = &is_one_of(&[
    ("red", Choice::Number(1.)),
    ("yellow", Choice::Number(2.)),
    ("green", Choice::Number(3.)),
    ("blue", Choice::Number(4.)),
    ("purple", Choice::Number(5.)),
    ("custom label", Choice::Text("custom")),
    ("none", Choice::Text("none")),
]);
const FILE_TYPE: &[Operation] = &is_one_of(&[
    ("Digital Negative (DNG)", Choice::Text("DNG")),
    ("Raw", Choice::Text("RAW")),
    ("HEIC", Choice::Text("HEIC")),
    ("JPEG", Choice::Text("JPG")),
    ("TIFF", Choice::Text("TIFF")),
    ("PNG", Choice::Text("PNG")),
    ("Photoshop Document (PSD)", Choice::Text("PSD")),
    ("Photoshop Large Document (PSB)", Choice::Text("PSB")),
    ("JPEG XL (JXL)", Choice::Text("JXL")),
    ("AVIF", Choice::Text("AVIF")),
    ("WEBP", Choice::Text("WEBP")),
    ("Video", Choice::Text("VIDEO")),
]);
const GPS: &[Operation] = &is_one_of(&[("Coordinates", Choice::Bool(true))]);
const TREATMENT: &[Operation] = &is_one_of(&[
    ("black and white", Choice::Text("grayscale")),
    ("color", Choice::Text("color")),
]);
const ASPECT: &[Operation] = &is_one_of(&[
    ("portrait", Choice::Text("portrait")),
    ("landscape", Choice::Text("landscape")),
    ("square", Choice::Text("square")),
]);
/// Any Searchable Text: Lightroom's menu offers any, all and none. Only
/// `any` was seen saved; `none` is taken to be stored as the other text
/// fields' doesn't contain.
const ALL_TEXT: &[Operation] = &[
    op("any", "any", Input::Text),
    op("all", "all", Input::Text),
    op("noneOf", "none", Input::Text),
];

const fn field(
    criteria: &'static str,
    name: &'static str,
    section: Option<&'static str>,
    operations: &'static [Operation],
) -> Field {
    Field {
        criteria,
        name,
        section,
        operations,
    }
}

/// The editor's fields, in Lightroom's menu order.
pub const FIELDS: &[Field] = &[
    field("rating", "Rating", None, RATING),
    field("pick", "Pick Flag", None, PICK),
    field("labelColor", "Label Color", None, LABEL_COLOR),
    field("labelText", "Label Text", None, LABEL_TEXT),
    field("folder", "Folder", Some("Source"), TEXT),
    field("collection", "Collection", Some("Source"), TEXT),
    field("filename", "Filename", Some("File Name / Type"), FILENAME),
    field("copyname", "Copy Name", Some("File Name / Type"), COPY_NAME),
    field(
        "fileFormat",
        "File Type",
        Some("File Name / Type"),
        FILE_TYPE,
    ),
    field("captureTime", "Capture Date", Some("Date"), DATE),
    field("touchTime", "Edit Date", Some("Date"), DATE),
    field("camera", "Camera", Some("Camera Info"), IS_OR_TEXT),
    field("lens", "Lens", Some("Camera Info"), IS_OR_TEXT),
    field("focalLength", "Focal Length", Some("Camera Info"), NUMBER),
    field(
        "shutterSpeed",
        "Shutter Speed",
        Some("Camera Info"),
        SHUTTER,
    ),
    field("aperture", "Aperture", Some("Camera Info"), APERTURE),
    field(
        "isoSpeedRating",
        "ISO Speed Rating",
        Some("Camera Info"),
        NUMBER,
    ),
    field("hasGPSData", "GPS Data", Some("Location"), GPS),
    field("title", "Title", Some("Other Metadata"), TEXT_OR_EMPTY),
    field("caption", "Caption", Some("Other Metadata"), TEXT_OR_EMPTY),
    field("keywords", "Keywords", Some("Other Metadata"), KEYWORDS),
    field("creator", "Creator", Some("Other Metadata"), IS_OR_TEXT),
    field(
        "metadata",
        "Any Searchable Metadata",
        Some("Other Metadata"),
        TEXT,
    ),
    field("hasAdjustments", "Has Adjustments", Some("Develop"), BOOL),
    field("cropped", "Cropped", Some("Develop"), BOOL),
    field("treatment", "Treatment", Some("Develop"), TREATMENT),
    field("longEdge", "Long Edge, Uncropped", Some("Size"), NUMBER),
    field("shortEdge", "Short Edge, Uncropped", Some("Size"), NUMBER),
    field("width", "Width, Uncropped", Some("Size"), NUMBER),
    field("height", "Height, Uncropped", Some("Size"), NUMBER),
    field("megapixels", "Megapixels, Uncropped", Some("Size"), NUMBER),
    field("aspectRatio", "Aspect Ratio", Some("Size"), ASPECT),
    field("all", "Any Searchable Text", None, ALL_TEXT),
];

/// The editor's field for `criteria`, if it offers it.
pub fn field_of(criteria: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.criteria == criteria)
}

impl Field {
    pub fn operation(&self, op: &str) -> Option<&'static Operation> {
        self.operations.iter().find(|o| o.op == op)
    }
    /// A new rule on this field: its first operation with a starting value,
    /// as Lightroom's editor shows one (Rating ≥ 0 stars… a date today).
    pub fn new_rule(&self, today: &str) -> Rule {
        let first = self.operations[0];
        let operation = match self.criteria {
            "rating" => self.operation(">=").unwrap_or(&first),
            _ => &first,
        };
        let mut rule = Rule::new(self.criteria, operation.op, None);
        set_starting_value(&mut rule, operation.input, today);
        rule
    }
}

/// Gives `rule` operation `op`, keeping its value when the new operation takes
/// the same kind, else a starting one.
pub fn set_operation(rule: &mut Rule, op: &Operation, today: &str) {
    let before = field_of(&rule.criteria)
        .and_then(|f| f.operation(&rule.operation))
        .map(|o| o.input);
    rule.operation = op.op.into();
    if before != Some(op.input) && !same_value(before, op.input) {
        set_starting_value(rule, op.input, today);
    }
    // A range starts at the value entered.
    if matches!(op.input, Input::Range(_)) && rule.value2.is_none() {
        rule.value2 = rule.value.clone();
    }
    if !matches!(op.input, Input::Period) {
        rule.units = None;
    }
}

/// Whether a value entered for `a` means the same for `b`: one value and a
/// range's first, of the same kind.
fn same_value(a: Option<Input>, b: Input) -> bool {
    match (a, b) {
        (Some(Input::Range(kind)), single) | (Some(single), Input::Range(kind)) => {
            kind.input() == single
        }
        _ => false,
    }
}

fn set_starting_value(rule: &mut Rule, input: Input, today: &str) {
    let (value, value2, units) = match input {
        Input::None => (None, None, None),
        Input::Text => (Some(Value::Str(String::new())), None, None),
        Input::Number | Input::Rating | Input::Shutter | Input::Aperture | Input::Date => {
            (Some(starting(input, today)), None, None)
        }
        Input::Range(kind) => {
            let value = starting(kind.input(), today);
            (Some(value.clone()), Some(value), None)
        }
        Input::Period => (Some(Value::Number(1.)), None, Some("days".to_string())),
        Input::Choice(choices) => (choices.first().map(|(_, c)| c.value()), None, None),
    };
    rule.value = value;
    rule.value2 = value2;
    rule.units = units;
}

/// One value's starting point: 0, 1/125 sec, f/5.6, today.
fn starting(input: Input, today: &str) -> Value {
    match input {
        Input::Shutter => Value::Number(125f64.log2()),
        Input::Aperture => Value::Number(2. * 5.6f64.log2()),
        Input::Date => Value::Str(today.into()),
        _ => Value::Number(0.),
    }
}

/// Whether the editor can show `rule` as a field, an operation and values
/// it offers; a rule it can't (an imported one with a value no menu lists,
/// or of another kind than the field takes) is shown read-only.
pub fn editable(rule: &Rule) -> bool {
    let Some(op) = field_of(&rule.criteria).and_then(|f| f.operation(&rule.operation)) else {
        return false;
    };
    let fits = |input: Input, value: &Option<Value>| match input {
        Input::None => true,
        Input::Text | Input::Date => matches!(value, Some(Value::Str(_)) | None),
        Input::Number | Input::Rating | Input::Shutter | Input::Aperture | Input::Period => {
            matches!(value, Some(Value::Number(_)))
        }
        Input::Choice(choices) => choices
            .iter()
            .any(|(_, c)| value.as_ref() == Some(&c.value())),
        Input::Range(_) => true,
    };
    match op.input {
        Input::Range(kind) => fits(kind.input(), &rule.value) && fits(kind.input(), &rule.value2),
        input => fits(input, &rule.value),
    }
}

/// Why a rule made in the editor can't be saved: a date that isn't one, a
/// period that isn't a whole number of days, weeks, months or years. None
/// when it can be.
pub fn invalid(rule: &Rule) -> Option<&'static str> {
    let op = field_of(&rule.criteria)?.operation(&rule.operation)?;
    let date = |v: &Option<Value>| matches!(v, Some(Value::Str(s)) if crate::collection_rules::Date::parse(s).is_some() && s.len() == 10);
    match op.input {
        Input::Date if !date(&rule.value) => Some("Enter dates as YYYY-MM-DD"),
        Input::Range(Single::Date) if !date(&rule.value) || !date(&rule.value2) => {
            Some("Enter dates as YYYY-MM-DD")
        }
        Input::Period => match rule.value {
            Some(Value::Number(n)) if n >= 0. && n.fract() == 0. && n <= 100_000. => None,
            _ => Some("A period is a whole number"),
        },
        Input::Rating => match rule.value {
            Some(Value::Number(n)) if (0. ..=5.).contains(&n) => None,
            _ => Some("A rating is 0 to 5 stars"),
        },
        _ => None,
    }
}

/// A shutter speed as the editor shows it, "1/250" or "2", from its APEX
/// value.
pub fn shutter_text(apex: f64) -> String {
    let seconds = (-apex).exp2();
    if seconds < 0.4 {
        format!("1/{}", trim((1. / seconds).round()))
    } else {
        trim((seconds * 10.).round() / 10.)
    }
}
/// The APEX value of a shutter speed entered as "1/250", "0.5" or "2".
pub fn shutter_apex(text: &str) -> Option<f64> {
    let text = text.trim().trim_end_matches("sec").trim();
    let seconds = match text.split_once('/') {
        Some((top, bottom)) => {
            top.trim().parse::<f64>().ok()? / bottom.trim().parse::<f64>().ok()?
        }
        None => text.parse::<f64>().ok()?,
    };
    (seconds > 0. && seconds.is_finite()).then(|| -seconds.log2())
}
/// An f-number as the editor shows it, from its APEX value.
pub fn aperture_text(apex: f64) -> String {
    trim(((apex / 2.).exp2() * 10.).round() / 10.)
}
/// The APEX value of an f-number entered as "5.6" or "f/5.6".
pub fn aperture_apex(text: &str) -> Option<f64> {
    let text = text
        .trim()
        .trim_start_matches(['f', 'F'])
        .trim_start_matches('/')
        .trim();
    let n = text.replace(',', ".").parse::<f64>().ok()?;
    (n > 0. && n.is_finite()).then(|| 2. * n.log2())
}

fn trim(n: f64) -> String {
    let text = format!("{n:.1}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection_rules::{Clock, Combine, Date, Group, Item};

    /// Every rule the editor can make is one RAWmakase evaluates.
    #[test]
    fn every_field_and_operation_offered_can_be_evaluated() {
        for field in FIELDS {
            for op in field.operations {
                let mut rule = field.new_rule("2026-11-17");
                set_operation(&mut rule, op, "2026-11-17");
                if let Input::Choice(choices) = op.input {
                    for (_, choice) in choices {
                        rule.value = Some(choice.value());
                        let group = Group::new(Combine::Intersect, vec![Item::Rule(rule.clone())]);
                        assert_eq!(
                            group.unsupported(),
                            None,
                            "{} {} {choice:?}",
                            field.name,
                            op.label
                        );
                    }
                }
                let group = Group::new(Combine::Intersect, vec![Item::Rule(rule)]);
                let clock = Clock {
                    today: Date::from_ymd(2026, 11, 17),
                    week_start: 0,
                };
                assert!(group.compile(&clock).is_ok(), "{} {}", field.name, op.label);
            }
        }
    }
    #[test]
    fn imported_values_no_menu_lists_are_read_only_and_bad_values_refused() {
        let rule = |criteria: &str, op: &str, value: Option<Value>| Rule::new(criteria, op, value);
        assert!(editable(&rule(
            "fileFormat",
            "==",
            Some(Value::Str("JPG".into()))
        )));
        assert!(!editable(&rule(
            "fileFormat",
            "==",
            Some(Value::Str("DNG-LOSSLESS".into()))
        )));
        assert!(!editable(&rule(
            "rating",
            ">=",
            Some(Value::Str("5".into()))
        )));
        assert!(!editable(&rule("likes", "isTrue", Some(Value::Bool(true)))));
        assert!(editable(&rule("title", "empty", None)));
        let mut date = rule("captureTime", "==", Some(Value::Str("2026-02-30".into())));
        assert!(invalid(&date).is_some());
        date.value = Some(Value::Str("2026-02-28".into()));
        assert_eq!(invalid(&date), None);
        let mut period = rule("captureTime", "inLast", Some(Value::Number(1.5)));
        assert!(invalid(&period).is_some());
        period.value = Some(Value::Number(-1.));
        assert!(invalid(&period).is_some());
        period.value = Some(Value::Number(3.));
        assert_eq!(invalid(&period), None);
        assert_eq!(field_of("all").unwrap().operations.len(), 3);
    }
    #[test]
    fn shutter_and_aperture_read_as_lightroom_stores_them() {
        assert!((shutter_apex("1/100").unwrap() - 6.6438561897747).abs() < 1e-9);
        assert_eq!(shutter_text(100f64.log2()), "1/100");
        assert_eq!(shutter_text(-1.), "2");
        assert!((aperture_apex("f/0,10").unwrap() + 6.6438561897747).abs() < 1e-9);
        assert_eq!(aperture_text(2. * 5.6f64.log2()), "5.6");
        assert_eq!(shutter_apex("0"), None);
        assert_eq!(aperture_apex("x"), None);
    }
    #[test]
    fn changing_operation_keeps_a_value_of_the_same_kind() {
        let title = field_of("title").unwrap();
        let mut rule = title.new_rule("2026-11-17");
        rule.value = Some(Value::Str("red".into()));
        set_operation(&mut rule, title.operation("words").unwrap(), "2026-11-17");
        assert_eq!(rule.value, Some(Value::Str("red".into())));
        set_operation(&mut rule, title.operation("empty").unwrap(), "2026-11-17");
        assert_eq!(rule.value, None);
        let date = field_of("captureTime").unwrap();
        let mut rule = date.new_rule("2026-11-17");
        set_operation(&mut rule, date.operation("inLast").unwrap(), "2026-11-17");
        assert_eq!(
            (rule.value.clone(), rule.units.as_deref()),
            (Some(Value::Number(1.)), Some("days"))
        );
        set_operation(&mut rule, date.operation("today").unwrap(), "2026-11-17");
        assert_eq!((rule.value, rule.units), (None, None));
        let rating = field_of("rating").unwrap().new_rule("");
        assert_eq!(rating.operation, ">=");
        // A shutter speed's range keeps its units and the speed entered.
        let shutter = field_of("shutterSpeed").unwrap();
        let mut rule = shutter.new_rule("2026-11-17");
        rule.value = Some(Value::Number(8.)); // 1/256 sec
        set_operation(&mut rule, shutter.operation("in").unwrap(), "2026-11-17");
        assert_eq!(
            shutter.operation("in").unwrap().input,
            Input::Range(Single::Shutter)
        );
        assert_eq!(
            (rule.value.clone(), rule.value2.clone()),
            (Some(Value::Number(8.)), Some(Value::Number(8.)))
        );
        set_operation(&mut rule, shutter.operation("<").unwrap(), "2026-11-17");
        assert_eq!(rule.value, Some(Value::Number(8.)));
    }
}
