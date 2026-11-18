//! Evaluating a smart collection's rules over [`Entry`] values, as Lightroom
//! Classic 15.6 was measured to (issue #998):
//!
//! - Text is compared case-insensitively. The value is split into words at
//!   white space. `any` (contains) matches when any word is a substring of the
//!   field ("red" finds Fred, reddish and red-car), `all` when every word is,
//!   `words` when every word is a whole word of the field (red-car is not
//!   "red"), `noneOf` when none is (an empty field matches), and `beginsWith`
//!   and `endsWith` look at each word of the field, not the whole field.
//! - Keywords match their ancestors, synonyms and keywords left out of export;
//!   Folder matches the whole path; Collection matches static collections'
//!   names, never smart ones', so a rule can't refer to itself or another
//!   smart collection and no cycle can form.
//! - Rating 0 is unrated. Dates are calendar days in local time: "in the last
//!   1 days" is today and yesterday, and a week starts on the clock's
//!   [`Clock::week_start`] (Sunday as measured on an en-US system).
//! - Shutter Speed and Aperture rules hold APEX values: Tv = −log₂(seconds),
//!   so 1/100 sec is 6.6438561897747, and Av = 2·log₂(f-number).
//!
//! Choices for what wasn't measured, each tested and listed in
//! docs/catalogs.md as a known difference until measured:
//!
//! - Words are split at Unicode white space only (and a folder's path also
//!   at its separators); punctuation stays part of a word, and letters are compared after Unicode lower-casing, without
//!   folding accents.
//! - Several words in `beginsWith` or `endsWith` must each begin (end) some
//!   word of the field. An empty value matches every photo, except with
//!   `empty` and `notEmpty`.
//! - A photo without a capture date (or edit date, the photo info a numeric
//!   rule asks about, or a GPS location the catalog keeps or cleared)
//!   matches no rule on it, `!=` and `notInLast`
//!   included. Dates after today are "in the last" any period.
//! - Label Text compares the whole label, case aside; `==` on Camera, Lens and
//!   Creator likewise.
//! - Collection rules see static collections' names, one per line, and Copy
//!   Name the copy name as stored (a master promoted from a copy keeps it, as
//!   measured).
use super::rules::{Combine, Group, Item, Rule};
use super::{Entry, LABELS, lua::Value};
use crate::time;
use std::path::Path;

/// A calendar day, as days since 1970-01-01.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(pub i64);
impl Date {
    pub fn from_ymd(year: i64, month: i64, day: i64) -> Self {
        Self(time::days(year, month, day))
    }
    pub fn ymd(self) -> (i64, i64, i64) {
        let [y, m, d, ..] = time::utc(self.0 * 86_400);
        (y, m, d)
    }
    /// The date at the start of `text`, "YYYY-MM-DD…" as the catalog keeps
    /// capture times and Lightroom writes rule values; None for anything else.
    pub fn parse(text: &str) -> Option<Self> {
        let b = text.as_bytes();
        if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let year: i64 = text.get(0..4)?.parse().ok()?;
        let month: i64 = text.get(5..7)?.parse().ok()?;
        let day: i64 = text.get(8..10)?.parse().ok()?;
        if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
            return None;
        }
        Some(Self::from_ymd(year, month, day))
    }
    /// 0 for Sunday … 6 for Saturday.
    pub fn weekday(self) -> i64 {
        (self.0 + 4).rem_euclid(7)
    }
    /// The same day `months` months earlier, the month's last day where it is
    /// shorter.
    fn months_back(self, months: i64) -> Self {
        let (y, m, d) = self.ymd();
        let index = y * 12 + (m - 1) - months;
        let (year, month) = (index.div_euclid(12), index.rem_euclid(12) + 1);
        Self::from_ymd(year, month, d.min(days_in_month(year, month)))
    }
}

fn days_in_month(year: i64, month: i64) -> i64 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    time::days(next_year, next_month, 1) - time::days(year, month, 1)
}

/// What relative date rules are relative to: today's local date and the
/// first day of the week, which the caller takes from the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    pub today: Date,
    /// 0 for Sunday … 6 for Saturday.
    pub week_start: i64,
}

/// Why a smart collection can't be evaluated: it lists no photos rather than
/// a partial match. `criteria` is empty when the rules as a whole can't be
/// read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported {
    pub criteria: String,
    pub operation: String,
    pub why: String,
}
impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.criteria.is_empty() {
            return f.write_str(&self.why);
        }
        let name = criteria_name(&self.criteria).unwrap_or(&self.criteria);
        write!(f, "{name}: {}", self.why)
    }
}
impl std::error::Error for Unsupported {}

/// Lightroom Classic 15.6's name for each criterion its smart collection
/// editor offers (`criteria` → menu label).
pub const CRITERIA: [(&str, &str); 78] = [
    ("filename", "Filename"),
    ("rating", "Rating"),
    ("lastExportTimestamp", "Exported"),
    ("pick", "Pick Flag"),
    ("labelColor", "Label Color"),
    ("labelText", "Label Text"),
    ("likes", "Has Likes"),
    ("comments", "Has Comments"),
    ("proxyStatus", "Has Smart Preview"),
    ("snapshots", "Has Snapshots"),
    ("folder", "Folder"),
    ("collection", "Collection"),
    ("publishCollection", "Publish Collection"),
    ("publishedVia", "Published Via"),
    ("copyname", "Copy Name"),
    ("fileFormat", "File Type"),
    ("fastLoadDNG", "Is DNG With Fast Load Data"),
    ("captureTime", "Capture Date"),
    ("touchTime", "Edit Date"),
    ("camera", "Camera"),
    ("cameraSN", "Camera Serial Number"),
    ("lens", "Lens"),
    ("focalLength", "Focal Length"),
    ("shutterSpeed", "Shutter Speed"),
    ("aperture", "Aperture"),
    ("isoSpeedRating", "ISO Speed Rating"),
    ("flashFired", "Flash State"),
    ("country", "Country / Region"),
    ("state", "State / Province"),
    ("city", "City"),
    ("location", "Sublocation"),
    ("hasGPSData", "GPS Data"),
    ("title", "Title"),
    ("caption", "Caption"),
    ("altText", "Alt Text"),
    ("extDescr", "Extended Description"),
    ("keywords", "Keywords"),
    ("creator", "Creator"),
    ("jobIdentifier", "Job"),
    ("copyrightState", "Copyright Status"),
    ("metadata", "Any Searchable Metadata"),
    ("iptc", "Searchable IPTC"),
    ("exif", "Searchable EXIF"),
    ("allPluginMetadata", "Any Searchable Plug-in Metadata"),
    ("metadataStatus", "Metadata Status"),
    ("hasAdjustments", "Has Adjustments"),
    ("edit", "Has Edits"),
    ("cropped", "Cropped"),
    ("hdrEditMode", "Is HDR Edit Mode On"),
    ("depthMap", "Has Device Depth"),
    ("hasAI", "Has AI"),
    ("hasGenAI", "Has Generative AI"),
    ("needsAIUpdate", "Needs AI Update"),
    ("aiEditStatus", "AI Edit"),
    ("aiUpdateRequiredStatus", "AI Edit Requiring Update"),
    ("maskTypeStatus", "Masking"),
    ("hasRetouch", "Remove (Manual)"),
    ("hasPointColor", "Has Point Color"),
    ("isProofCopy", "Is Proof"),
    ("developPreset", "Develop Preset"),
    ("treatment", "Treatment"),
    ("profileCorrections", "Lens Profile Corrections"),
    ("removeChromaticAberration", "Remove Chromatic Aberration"),
    ("longEdgeCropped", "Long Edge"),
    ("shortEdgeCropped", "Short Edge"),
    ("widthCropped", "Width"),
    ("heightCropped", "Height"),
    ("megapixelsCropped", "Megapixels"),
    ("longEdge", "Long Edge, Uncropped"),
    ("shortEdge", "Short Edge, Uncropped"),
    ("width", "Width, Uncropped"),
    ("height", "Height, Uncropped"),
    ("megapixels", "Megapixels, Uncropped"),
    ("aspectRatio", "Aspect Ratio"),
    ("bitDepth", "Bits Per Channel"),
    ("colorMode", "Source Color Mode"),
    ("colorProfile", "Source Color Profile"),
    ("all", "Any Searchable Text"),
];

/// Lightroom's menu name for `criteria`.
pub fn criteria_name(criteria: &str) -> Option<&'static str> {
    CRITERIA
        .iter()
        .find(|(c, _)| *c == criteria)
        .map(|(_, name)| *name)
}

/// What a criterion is about, and so which operations it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Text(TextField),
    LabelText,
    Number(NumberField),
    Pick,
    LabelColor,
    FileFormat,
    Treatment,
    AspectRatio,
    Flag(FlagField),
    Gps,
    Date(DateField),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextField {
    Filename,
    CopyName,
    Folder,
    Collection,
    Camera,
    Lens,
    Title,
    Caption,
    Keywords,
    Creator,
    Metadata,
    All,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumberField {
    Rating,
    FocalLength,
    ShutterSpeed,
    Aperture,
    Iso,
    Width,
    Height,
    LongEdge,
    ShortEdge,
    Megapixels,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FlagField {
    HasAdjustments,
    Cropped,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DateField {
    Capture,
    Edit,
}

fn field(criteria: &str) -> Option<Field> {
    use Field::*;
    Some(match criteria {
        "filename" => Text(TextField::Filename),
        "copyname" => Text(TextField::CopyName),
        "folder" => Text(TextField::Folder),
        "collection" => Text(TextField::Collection),
        "camera" => Text(TextField::Camera),
        "lens" => Text(TextField::Lens),
        "title" => Text(TextField::Title),
        "caption" => Text(TextField::Caption),
        "keywords" => Text(TextField::Keywords),
        "creator" => Text(TextField::Creator),
        "metadata" => Text(TextField::Metadata),
        "all" => Text(TextField::All),
        "labelText" => LabelText,
        "rating" => Number(NumberField::Rating),
        "focalLength" => Number(NumberField::FocalLength),
        "shutterSpeed" => Number(NumberField::ShutterSpeed),
        "aperture" => Number(NumberField::Aperture),
        "isoSpeedRating" => Number(NumberField::Iso),
        "width" => Number(NumberField::Width),
        "height" => Number(NumberField::Height),
        "longEdge" => Number(NumberField::LongEdge),
        "shortEdge" => Number(NumberField::ShortEdge),
        "megapixels" => Number(NumberField::Megapixels),
        "pick" => Pick,
        "labelColor" => LabelColor,
        "fileFormat" => FileFormat,
        "treatment" => Treatment,
        "aspectRatio" => AspectRatio,
        "hasAdjustments" => Flag(FlagField::HasAdjustments),
        "cropped" => Flag(FlagField::Cropped),
        "hasGPSData" => Gps,
        "captureTime" => Date(DateField::Capture),
        "touchTime" => Date(DateField::Edit),
        _ => return None,
    })
}

/// The operations Lightroom's editor offers for each text field.
fn text_operations(field: TextField) -> &'static [&'static str] {
    const SEARCH: [&str; 6] = ["any", "all", "words", "noneOf", "beginsWith", "endsWith"];
    match field {
        TextField::Filename => &["any", "all", "noneOf", "beginsWith", "endsWith"],
        TextField::CopyName => &[
            "any",
            "all",
            "noneOf",
            "beginsWith",
            "endsWith",
            "empty",
            "notEmpty",
        ],
        TextField::Folder | TextField::Collection | TextField::Metadata => &SEARCH,
        TextField::Camera | TextField::Lens | TextField::Creator => &[
            "==",
            "!=",
            "any",
            "all",
            "words",
            "noneOf",
            "beginsWith",
            "endsWith",
        ],
        TextField::Title | TextField::Caption | TextField::Keywords => &[
            "any",
            "all",
            "words",
            "noneOf",
            "beginsWith",
            "endsWith",
            "empty",
            "notEmpty",
        ],
        // Lightroom's menu shows any, all and none; only `any` was seen saved.
        TextField::All => &["any", "all", "words", "noneOf"],
    }
}

/// The text compiled once: lower-cased words, or the whole value.
#[derive(Clone, Debug)]
enum TextTest {
    Any(Vec<String>),
    All(Vec<String>),
    Words(Vec<String>),
    NoneOf(Vec<String>),
    BeginsWith(Vec<String>),
    EndsWith(Vec<String>),
    Equal(String),
    NotEqual(String),
    Empty,
    NotEmpty,
}
impl TextTest {
    fn new(operation: &str, value: &str) -> Option<Self> {
        let lower = value.to_lowercase();
        let words = || lower.split_whitespace().map(str::to_string).collect();
        Some(match operation {
            "any" => Self::Any(words()),
            "all" => Self::All(words()),
            "words" => Self::Words(words()),
            "noneOf" => Self::NoneOf(words()),
            "beginsWith" => Self::BeginsWith(words()),
            "endsWith" => Self::EndsWith(words()),
            "==" => Self::Equal(lower.trim().to_string()),
            "!=" => Self::NotEqual(lower.trim().to_string()),
            "empty" => Self::Empty,
            "notEmpty" => Self::NotEmpty,
            _ => return None,
        })
    }
    /// Whether a folder's path passes: its folders' names are words too, so
    /// "words trips" finds /photos/trips/Kyoto, while contains still sees the
    /// whole path.
    fn holds_path(&self, path: &str) -> bool {
        match self {
            Self::Words(_) | Self::BeginsWith(_) | Self::EndsWith(_) => {
                self.holds(&path.replace(['/', '\\'], " "))
            }
            _ => self.holds(path),
        }
    }
    /// Whether `text` passes, lines being separate values (keywords,
    /// collections, creators).
    fn holds(&self, text: &str) -> bool {
        let empty = text.trim().is_empty();
        match self {
            Self::Empty => return empty,
            Self::NotEmpty => return !empty,
            _ => {}
        }
        let text = text.to_lowercase();
        let field_words = || text.split_whitespace();
        let lines = || text.lines().map(str::trim);
        match self {
            Self::Any(w) => w.is_empty() || w.iter().any(|w| text.contains(w.as_str())),
            Self::All(w) => w.iter().all(|w| text.contains(w.as_str())),
            Self::Words(w) => w.iter().all(|w| field_words().any(|f| f == w)),
            Self::NoneOf(w) => !w.iter().any(|w| text.contains(w.as_str())),
            Self::BeginsWith(w) => w
                .iter()
                .all(|w| field_words().any(|f| f.starts_with(w.as_str()))),
            Self::EndsWith(w) => w
                .iter()
                .all(|w| field_words().any(|f| f.ends_with(w.as_str()))),
            Self::Equal(v) => lines().any(|l| l == v),
            Self::NotEqual(v) => !lines().any(|l| l == v),
            Self::Empty | Self::NotEmpty => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum NumberOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
    In,
}

/// Equal within this, so a rule's 14-digit APEX value finds the exposure it
/// was made from.
const EPSILON: f64 = 1e-6;

#[derive(Clone, Copy, Debug)]
enum LabelColor {
    Color(&'static str),
    Custom,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Aspect {
    Portrait,
    Landscape,
    Square,
}

/// Days a date rule lets through, inclusive; `negate` for `!=` and
/// `notInLast`.
#[derive(Clone, Copy, Debug)]
struct DateRange {
    from: Option<Date>,
    to: Option<Date>,
    negate: bool,
}

#[derive(Clone, Debug)]
enum Test {
    Text(TextField, TextTest),
    LabelText(TextTest),
    Number(NumberField, NumberOp, f64, f64),
    Pick(bool, i32),
    LabelColor(bool, LabelColor),
    FileFormat(bool, String),
    Treatment(bool, bool),
    AspectRatio(bool, Aspect),
    Flag(FlagField, bool),
    Gps(bool, bool),
    Date(DateField, DateRange),
}

/// Rules ready to test entries with, for one [`Clock`].
#[derive(Clone, Debug)]
pub struct Matcher {
    combine: Combine,
    items: Vec<Node>,
}
#[derive(Clone, Debug)]
enum Node {
    Test(Test),
    Group(Matcher),
}

impl Group {
    /// The first rule that can't be evaluated, if any.
    pub fn unsupported(&self) -> Option<Unsupported> {
        let clock = Clock {
            today: Date(0),
            week_start: 0,
        };
        self.compile(&clock).err()
    }
    /// These rules ready to test entries with, relative dates taken from
    /// `clock`; the first rule that can't be evaluated otherwise.
    pub fn compile(&self, clock: &Clock) -> Result<Matcher, Unsupported> {
        Ok(Matcher {
            combine: self.combine,
            items: self
                .items
                .iter()
                .map(|item| match item {
                    Item::Rule(rule) => compile_rule(rule, clock).map(Node::Test),
                    Item::Group(group) => group.compile(clock).map(Node::Group),
                })
                .collect::<Result<_, _>>()?,
        })
    }
    /// Whether testing needs [`Entry::edit`] or [`Entry::edited`], which
    /// cost a catalog read.
    pub fn needs_edit_facts(&self) -> bool {
        self.rules().iter().any(|r| {
            matches!(
                field(&r.criteria),
                Some(Field::Flag(_) | Field::Treatment | Field::Date(DateField::Edit))
            )
        })
    }
    /// Whether a rule reads keywords, whose Lightroom synonyms a catalog
    /// imported earlier may not have.
    pub fn reads_keywords(&self) -> bool {
        self.rules()
            .iter()
            .any(|r| matches!(r.criteria.as_str(), "keywords" | "metadata" | "all"))
    }
}

fn compile_rule(rule: &Rule, clock: &Clock) -> Result<Test, Unsupported> {
    let unsupported = |why: &str| Unsupported {
        criteria: rule.criteria.clone(),
        operation: rule.operation.clone(),
        why: why.into(),
    };
    let Some(field) = field(&rule.criteria) else {
        return Err(unsupported(if criteria_name(&rule.criteria).is_some() {
            "RAWmakase doesn't keep what this rule asks about"
        } else {
            "not a rule RAWmakase knows"
        }));
    };
    let op = rule.operation.as_str();
    let unknown_op = || {
        unsupported(&format!(
            "the operation \"{op}\" isn't known for this field"
        ))
    };
    fn text_of(value: &Option<Value>) -> Option<&str> {
        match value {
            Some(Value::Str(s)) => Some(s.as_str()),
            None => Some(""),
            Some(_) => None,
        }
    }
    let text = |value| text_of(value).ok_or_else(|| unsupported("its value is not text"));
    let number = |value: &Option<Value>| match value {
        Some(Value::Number(n)) => Ok(*n),
        _ => Err(unsupported("its value is not a number")),
    };
    let negate = op == "!=";
    let equality = || match op {
        "==" | "!=" => Ok(op == "!="),
        _ => Err(unknown_op()),
    };
    Ok(match field {
        Field::Text(f) => {
            if !text_operations(f).contains(&op) {
                return Err(unknown_op());
            }
            Test::Text(
                f,
                TextTest::new(op, text(&rule.value)?).ok_or_else(unknown_op)?,
            )
        }
        Field::LabelText => {
            if !["==", "!=", "empty", "notEmpty"].contains(&op) {
                return Err(unknown_op());
            }
            Test::LabelText(TextTest::new(op, text(&rule.value)?).ok_or_else(unknown_op)?)
        }
        Field::Number(f) => {
            let op = match op {
                "==" => NumberOp::Eq,
                "!=" => NumberOp::Ne,
                ">" => NumberOp::Gt,
                "<" => NumberOp::Lt,
                ">=" => NumberOp::Ge,
                "<=" => NumberOp::Le,
                "in" => NumberOp::In,
                _ => return Err(unknown_op()),
            };
            let a = number(&rule.value)?;
            let b = match op {
                NumberOp::In => number(&rule.value2)?,
                _ => a,
            };
            Test::Number(f, op, a.min(b), a.max(b))
        }
        Field::Pick => {
            let n = number(&rule.value)?;
            if ![-1., 0., 1.].contains(&n) {
                return Err(unsupported("its flag is not known"));
            }
            #[allow(clippy::cast_possible_truncation)]
            Test::Pick(equality()?, n as i32)
        }
        Field::LabelColor => {
            let color = match &rule.value {
                Some(Value::Number(n)) if (1. ..=5.).contains(n) && n.fract() == 0. =>
                {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    LabelColor::Color(LABELS[*n as usize - 1])
                }
                Some(Value::Str(s)) if s == "custom" => LabelColor::Custom,
                Some(Value::Str(s)) if s == "none" => LabelColor::None,
                _ => return Err(unsupported("its label is not known")),
            };
            Test::LabelColor(equality()?, color)
        }
        Field::FileFormat => {
            let value = text(&rule.value)?;
            if !FILE_FORMATS.contains(&value) {
                return Err(unsupported(&format!(
                    "RAWmakase can't tell \"{value}\" files apart"
                )));
            }
            Test::FileFormat(equality()?, value.to_string())
        }
        Field::Treatment => {
            let grayscale = match text(&rule.value)? {
                "grayscale" => true,
                "color" => false,
                _ => return Err(unsupported("its treatment is not known")),
            };
            Test::Treatment(equality()?, grayscale)
        }
        Field::AspectRatio => {
            let aspect = match text(&rule.value)? {
                "portrait" => Aspect::Portrait,
                "landscape" => Aspect::Landscape,
                "square" => Aspect::Square,
                _ => return Err(unsupported("its aspect ratio is not known")),
            };
            Test::AspectRatio(equality()?, aspect)
        }
        Field::Flag(f) => {
            let want = match op {
                "isTrue" => true,
                "isFalse" => false,
                _ => return Err(unknown_op()),
            };
            if !matches!(rule.value, None | Some(Value::Bool(true))) {
                return Err(unsupported("its value is not known"));
            }
            Test::Flag(f, want)
        }
        Field::Gps => {
            let Some(Value::Bool(want)) = rule.value else {
                return Err(unsupported("its value is not known"));
            };
            Test::Gps(equality()?, want)
        }
        Field::Date(f) => {
            let date = |value: &Option<Value>| {
                text_of(value)
                    .and_then(Date::parse)
                    .ok_or_else(|| unsupported("its date is not known"))
            };
            let today = clock.today;
            let range = |from, to, negate| DateRange { from, to, negate };
            let day = |d: Date| range(Some(d), Some(d), false);
            Test::Date(
                f,
                match op {
                    "==" | "!=" => {
                        let d = date(&rule.value)?;
                        range(Some(d), Some(d), negate)
                    }
                    ">" => range(Some(Date(date(&rule.value)?.0 + 1)), None, false),
                    "<" => range(None, Some(Date(date(&rule.value)?.0 - 1)), false),
                    "in" => {
                        let (a, b) = (date(&rule.value)?, date(&rule.value2)?);
                        range(Some(a.min(b)), Some(a.max(b)), false)
                    }
                    "inLast" | "notInLast" => {
                        let n = number(&rule.value)?;
                        if n < 0. || n.fract() != 0. || n > 100_000. {
                            return Err(unsupported("its period is not known"));
                        }
                        #[allow(clippy::cast_possible_truncation)]
                        let n = n as i64;
                        let from = match rule.units.as_deref() {
                            Some("days") => Date(today.0 - n),
                            Some("weeks") => Date(today.0 - 7 * n),
                            Some("months") => today.months_back(n),
                            Some("years") => today.months_back(12 * n),
                            _ => return Err(unsupported("its period's unit is not known")),
                        };
                        range(Some(from), None, op == "notInLast")
                    }
                    "today" => day(today),
                    "yesterday" => day(Date(today.0 - 1)),
                    "thisWeek" => {
                        let start =
                            Date(today.0 - (today.weekday() - clock.week_start).rem_euclid(7));
                        range(Some(start), Some(Date(start.0 + 6)), false)
                    }
                    "thisMonth" => {
                        let (y, m, _) = today.ymd();
                        let start = Date::from_ymd(y, m, 1);
                        range(
                            Some(start),
                            Some(Date(start.0 + days_in_month(y, m) - 1)),
                            false,
                        )
                    }
                    "thisYear" => {
                        let (y, _, _) = today.ymd();
                        range(
                            Some(Date::from_ymd(y, 1, 1)),
                            Some(Date::from_ymd(y, 12, 31)),
                            false,
                        )
                    }
                    _ => return Err(unknown_op()),
                },
            )
        }
    })
}

/// Lightroom's File Type values RAWmakase tells apart. DNG's lossless,
/// lossy and reduced-resolution kinds aren't among them.
const FILE_FORMATS: [&str; 12] = [
    "DNG", "RAW", "HEIC", "JPG", "TIFF", "PNG", "PSD", "PSB", "JXL", "AVIF", "WEBP", "VIDEO",
];

/// Lightroom's File Type value for `filename`, by its extension.
fn file_format(filename: &str) -> Option<&'static str> {
    let (_, extension) = filename.rsplit_once('.')?;
    let is = |names: &[&str]| names.iter().any(|n| extension.eq_ignore_ascii_case(n));
    Some(if is(&["dng"]) {
        "DNG"
    } else if crate::storage::is_raw(Path::new(filename)) {
        "RAW"
    } else if is(&["jpg", "jpeg", "jpe"]) {
        "JPG"
    } else if is(&["tif", "tiff"]) {
        "TIFF"
    } else if is(&["heic", "heif", "hif"]) {
        "HEIC"
    } else if is(&["png"]) {
        "PNG"
    } else if is(&["psd"]) {
        "PSD"
    } else if is(&["psb"]) {
        "PSB"
    } else if is(&["jxl"]) {
        "JXL"
    } else if is(&["avif"]) {
        "AVIF"
    } else if is(&["webp"]) {
        "WEBP"
    } else if is(&[
        "mov", "mp4", "m4v", "avi", "mts", "m2ts", "mpg", "mpeg", "3gp",
    ]) {
        "VIDEO"
    } else {
        return None;
    })
}

impl Matcher {
    pub fn matches(&self, entry: &Entry<'_>) -> bool {
        let hit = |node: &Node| match node {
            Node::Test(test) => test.holds(entry),
            Node::Group(group) => group.matches(entry),
        };
        match self.combine {
            Combine::Union => self.items.iter().any(hit),
            Combine::Intersect => self.items.iter().all(hit),
            Combine::Exclude => !self.items.iter().any(hit),
        }
    }
}

impl Test {
    fn holds(&self, e: &Entry<'_>) -> bool {
        match self {
            Self::Text(field, test) => match field {
                TextField::Filename => test.holds(e.filename),
                TextField::CopyName => test.holds(e.copy_name),
                TextField::Folder => test.holds_path(e.folder),
                TextField::Collection => test.holds(e.collections),
                TextField::Camera => test.holds(info_text(e, |i| i.camera.as_deref())),
                TextField::Lens => test.holds(info_text(e, |i| i.lens.as_deref())),
                TextField::Title => test.holds(e.title),
                TextField::Caption => test.holds(e.caption),
                TextField::Keywords => test.holds(e.keyword_terms),
                TextField::Creator => test.holds(e.creator),
                TextField::Metadata => test.holds(&searchable_metadata(e)),
                TextField::All => test.holds(&searchable_text(e)),
            },
            Self::LabelText(test) => test.holds(e.label),
            Self::Number(field, op, a, b) => number(e, *field).is_some_and(|x| match op {
                NumberOp::Eq => (x - a).abs() <= EPSILON,
                NumberOp::Ne => (x - a).abs() > EPSILON,
                NumberOp::Gt => x > a + EPSILON,
                NumberOp::Lt => x < a - EPSILON,
                NumberOp::Ge => x >= a - EPSILON,
                NumberOp::Le => x <= a + EPSILON,
                NumberOp::In => x >= a - EPSILON && x <= b + EPSILON,
            }),
            Self::Pick(negate, flag) => (e.flag == *flag) != *negate,
            Self::LabelColor(negate, color) => {
                let is = match color {
                    LabelColor::Color(name) => e.label == *name,
                    LabelColor::Custom => !e.label.is_empty() && !LABELS.contains(&e.label),
                    LabelColor::None => e.label.is_empty(),
                };
                is != *negate
            }
            Self::FileFormat(negate, format) => {
                (file_format(e.filename) == Some(format.as_str())) != *negate
            }
            Self::Treatment(negate, grayscale) => (e.edit.grayscale == *grayscale) != *negate,
            Self::AspectRatio(negate, aspect) => {
                e.info.and_then(|i| i.dimensions).is_some_and(|(w, h)| {
                    let is = match w.cmp(&h) {
                        std::cmp::Ordering::Less => Aspect::Portrait,
                        std::cmp::Ordering::Greater => Aspect::Landscape,
                        std::cmp::Ordering::Equal => Aspect::Square,
                    };
                    (is == *aspect) != *negate
                })
            }
            Self::Flag(field, want) => {
                let is = match field {
                    FlagField::HasAdjustments => e.edit.adjusted,
                    FlagField::Cropped => e.edit.cropped,
                };
                is == *want
            }
            Self::Gps(negate, want) => e.gps.is_some_and(|has| (has == *want) != *negate),
            Self::Date(field, range) => {
                let date = match field {
                    DateField::Capture => Date::parse(e.captured),
                    DateField::Edit => e.edited,
                };
                date.is_some_and(|d| {
                    let inside =
                        range.from.is_none_or(|f| d >= f) && range.to.is_none_or(|t| d <= t);
                    inside != range.negate
                })
            }
        }
    }
}

fn info_text<'a>(
    e: &Entry<'a>,
    get: impl Fn(&'a crate::metadata::PhotoInfo) -> Option<&'a str>,
) -> &'a str {
    e.info.and_then(get).unwrap_or("")
}

fn number(e: &Entry<'_>, field: NumberField) -> Option<f64> {
    let info = e.info;
    let dimensions = || {
        info.and_then(|i| i.dimensions)
            .map(|(w, h)| (f64::from(w), f64::from(h)))
    };
    match field {
        NumberField::Rating => Some(f64::from(e.rating)),
        NumberField::FocalLength => info?.focal.filter(|f| *f > 0.),
        NumberField::ShutterSpeed => info?.exposure.filter(|t| *t > 0.).map(|t| -t.log2()),
        NumberField::Aperture => info?.aperture.filter(|f| *f > 0.).map(|f| 2. * f.log2()),
        NumberField::Iso => info?.iso.filter(|i| *i > 0.),
        NumberField::Width => dimensions().map(|(w, _)| w),
        NumberField::Height => dimensions().map(|(_, h)| h),
        NumberField::LongEdge => dimensions().map(|(w, h)| w.max(h)),
        NumberField::ShortEdge => dimensions().map(|(w, h)| w.min(h)),
        NumberField::Megapixels => dimensions().map(|(w, h)| w * h / 1e6),
    }
}

/// Any Searchable Metadata: title, caption, copyright, creator, keywords
/// (with their ancestors and synonyms), label, camera and lens, one per line.
fn searchable_metadata(e: &Entry<'_>) -> String {
    [
        e.title,
        e.caption,
        e.copyright,
        e.creator,
        e.keyword_terms,
        e.label,
        info_text(e, |i| i.camera.as_deref()),
        info_text(e, |i| i.lens.as_deref()),
    ]
    .join("\n")
}

/// Any Searchable Text: the searchable metadata with the file name, copy
/// name, folder path and static collections' names.
fn searchable_text(e: &Entry<'_>) -> String {
    [
        searchable_metadata(e).as_str(),
        e.filename,
        e.copy_name,
        e.folder,
        e.collections,
    ]
    .join("\n")
}

#[cfg(test)]
#[path = "eval_tests.rs"]
mod tests;
