//! Lightroom's Filename Template Editor: a file name built from tokens, as
//! Export's Rename To > Edit… makes it.
//!
//! A template is kept as Lightroom keeps it (`AgExport_tokens`, a
//! `.lrtemplate`'s tokens): literal text with `{{id}}` tokens, such as
//! `{{date_YYYYMMDD}}-{{image_name}}`. [`TokenTemplate::parse`] reads any text
//! and [`TokenTemplate::to_lightroom`] writes back exactly what it read, ids
//! RAWmakase doesn't know included. The editor shows another form, with the
//! tokens by their menu labels (`{Date (YYYYMMDD)}-{Filename}`), which reads
//! back to the same template ([`TokenTemplate::to_display`],
//! [`TokenTemplate::from_display`]).
use super::{NameContext, name_part, reserved};
use std::ops::Range;
use std::path::Path;

/// What a token stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    /// The file's name without its extension.
    Filename,
    /// The digits at the end of the file's name ("0001" of DSC0001).
    FilenameNumberSuffix,
    /// The name its file had when it entered the catalog, without its
    /// extension (see [`super::NameFields::original_name`]).
    OriginalFilename,
    /// A virtual copy's name.
    CopyName,
    /// The name of the folder the file is in.
    FolderName,
    /// The photo's number in the export, from Start Number, with at least
    /// this many digits.
    Sequence(u8),
    /// How many photos the export has, with at least this many digits.
    Total(u8),
    /// A part of the capture date.
    Date(DatePart),
    Field(Field),
    /// The Custom Text field.
    CustomText,
    /// A token of Lightroom's that RAWmakase doesn't fill yet, by its label.
    Unsupported(&'static str),
}

/// The parts of a capture date Lightroom's date tokens name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatePart {
    /// "20260424"
    Yyyymmdd,
    /// "260424"
    Yymmdd,
    Yyyy,
    Yy,
    /// "April"
    Month,
    /// "Apr"
    Mon,
    Mm,
    Dd,
    /// The day of the year, "114".
    Julian,
    Hour,
    Minute,
    Second,
    /// "April 24, 2026"
    Long,
}
impl DatePart {
    /// Whether it needs the time of day, not only the date.
    fn time(self) -> bool {
        matches!(self, Self::Hour | Self::Minute | Self::Second)
    }
}

/// The photo's metadata Lightroom's metadata tokens name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Title,
    Caption,
    Copyright,
    Creator,
    Rating,
    Label,
    /// The camera's maker, as RAWmakase spells it ("Sony").
    Make,
    /// The camera's model as the file writes it ("ILCE-7RM6").
    Model,
    Lens,
    Iso,
    /// "1/250 sec at f / 2.8"
    Exposure,
    FocalLength,
    /// "6000 x 4000"
    Dimensions,
    /// The software that wrote the file ("DSLR-A230 v1.10").
    Software,
    /// "39 mm"
    FocalLength35,
    /// Its first creator.
    Artist,
    /// Its keywords, by name.
    Keywords,
    Headline,
    City,
    State,
    Country,
    Sublocation,
}

/// Where the editor offers a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    ImageName,
    Numbering,
    Date,
    /// Lightroom's first Metadata popup: the IPTC fields.
    Metadata,
    /// Its second: the EXIF fields, rating and label.
    Exif,
    Custom,
}

/// A token Lightroom knows: its id, the label its menu shows, and what it
/// stands for.
#[derive(Clone, Copy, Debug)]
pub struct Def {
    pub id: &'static str,
    pub label: &'static str,
    pub token: Token,
    /// `None` for the tokens RAWmakase doesn't fill yet, which the editor
    /// shows when a template has them but doesn't offer.
    pub menu: Option<Menu>,
}

const fn def(id: &'static str, label: &'static str, token: Token, menu: Menu) -> Def {
    Def {
        id,
        label,
        token,
        menu: Some(menu),
    }
}
const fn unsupported(id: &'static str, label: &'static str) -> Def {
    Def {
        id,
        label,
        token: Token::Unsupported(label),
        menu: None,
    }
}

/// Lightroom Classic's file-name tokens (ids as its `AgNamingEvaluators` and
/// token tag sets name them, labels as its menus show them), in menu order.
/// Labels are unique: the editor reads a token back by its label.
pub const TOKENS: &[Def] = &[
    def("image_name", "Filename", Token::Filename, Menu::ImageName),
    def(
        "image_filename_number_suffix",
        "Filename number suffix",
        Token::FilenameNumberSuffix,
        Menu::ImageName,
    ),
    def("copy_name", "Copy name", Token::CopyName, Menu::ImageName),
    def(
        "image_folder",
        "Folder Name",
        Token::FolderName,
        Menu::ImageName,
    ),
    def(
        "image_originalName",
        "Original filename",
        Token::OriginalFilename,
        Menu::ImageName,
    ),
    unsupported("image_originalName_number_suffix", "Original number suffix"),
    unsupported("image_preservedName", "Preserved filename"),
    unsupported("image_name_extension", "Filename extension"),
    def(
        "naming_sequenceNumber_1Digit",
        "Sequence # (1)",
        Token::Sequence(1),
        Menu::Numbering,
    ),
    def(
        "naming_sequenceNumber_2Digits",
        "Sequence # (01)",
        Token::Sequence(2),
        Menu::Numbering,
    ),
    def(
        "naming_sequenceNumber_3Digits",
        "Sequence # (001)",
        Token::Sequence(3),
        Menu::Numbering,
    ),
    def(
        "naming_sequenceNumber_4Digits",
        "Sequence # (0001)",
        Token::Sequence(4),
        Menu::Numbering,
    ),
    def(
        "naming_sequenceNumber_5Digits",
        "Sequence # (00001)",
        Token::Sequence(5),
        Menu::Numbering,
    ),
    def(
        "naming_sequenceTotal_1Digit",
        "Total # (1)",
        Token::Total(1),
        Menu::Numbering,
    ),
    def(
        "naming_sequenceTotal_2Digits",
        "Total # (01)",
        Token::Total(2),
        Menu::Numbering,
    ),
    def(
        "naming_sequenceTotal_3Digits",
        "Total # (001)",
        Token::Total(3),
        Menu::Numbering,
    ),
    def(
        "naming_sequenceTotal_4Digits",
        "Total # (0001)",
        Token::Total(4),
        Menu::Numbering,
    ),
    def(
        "naming_sequenceTotal_5Digits",
        "Total # (00001)",
        Token::Total(5),
        Menu::Numbering,
    ),
    unsupported("naming_importNumber_1Digit", "Import # (1)"),
    unsupported("naming_importNumber_2Digits", "Import # (01)"),
    unsupported("naming_importNumber_3Digits", "Import # (001)"),
    unsupported("naming_importNumber_4Digits", "Import # (0001)"),
    unsupported("naming_importNumber_5Digits", "Import # (00001)"),
    unsupported("naming_operationSequence_1Digit", "Image # (1)"),
    unsupported("naming_operationSequence_2Digits", "Image # (01)"),
    unsupported("naming_operationSequence_3Digits", "Image # (001)"),
    unsupported("naming_operationSequence_4Digits", "Image # (0001)"),
    unsupported("naming_operationSequence_5Digits", "Image # (00001)"),
    def(
        "date_YYYYMMDD",
        "Date (YYYYMMDD)",
        Token::Date(DatePart::Yyyymmdd),
        Menu::Date,
    ),
    def(
        "date_YYMMDD",
        "Date (YYMMDD)",
        Token::Date(DatePart::Yymmdd),
        Menu::Date,
    ),
    def(
        "date_YYYY",
        "Date (YYYY)",
        Token::Date(DatePart::Yyyy),
        Menu::Date,
    ),
    def(
        "date_YY",
        "Date (YY)",
        Token::Date(DatePart::Yy),
        Menu::Date,
    ),
    def(
        "date_Month",
        "Date (Month)",
        Token::Date(DatePart::Month),
        Menu::Date,
    ),
    def(
        "date_Mon",
        "Date (Mon)",
        Token::Date(DatePart::Mon),
        Menu::Date,
    ),
    def(
        "date_MM",
        "Date (MM)",
        Token::Date(DatePart::Mm),
        Menu::Date,
    ),
    def(
        "date_DD",
        "Date (DD)",
        Token::Date(DatePart::Dd),
        Menu::Date,
    ),
    def(
        "date_LocalEncoding",
        "Date (Month DD, YYYY)",
        Token::Date(DatePart::Long),
        Menu::Date,
    ),
    def(
        "date_Julian",
        "Julian Day of the Year",
        Token::Date(DatePart::Julian),
        Menu::Date,
    ),
    def("date_Hour", "Hour", Token::Date(DatePart::Hour), Menu::Date),
    def(
        "date_Minute",
        "Minute",
        Token::Date(DatePart::Minute),
        Menu::Date,
    ),
    def(
        "date_Second",
        "Second",
        Token::Date(DatePart::Second),
        Menu::Date,
    ),
    def(
        "com.adobe.title",
        "Title",
        Token::Field(Field::Title),
        Menu::Metadata,
    ),
    def(
        "com.adobe.caption",
        "Caption",
        Token::Field(Field::Caption),
        Menu::Metadata,
    ),
    def(
        "com.adobe.copyright",
        "Copyright",
        Token::Field(Field::Copyright),
        Menu::Metadata,
    ),
    def(
        "com.adobe.keywords",
        "Keywords",
        Token::Field(Field::Keywords),
        Menu::Metadata,
    ),
    def(
        "com.adobe.creator",
        "Creator",
        Token::Field(Field::Creator),
        Menu::Metadata,
    ),
    unsupported("com.adobe.creatorJobTitle", "Creator Job Title"),
    unsupported("com.adobe.creatorAddress", "Creator Address"),
    unsupported("com.adobe.creatorCity", "Creator City"),
    unsupported("com.adobe.creatorState", "Creator State / Province"),
    unsupported("com.adobe.creatorZip", "Creator Postal Code"),
    unsupported("com.adobe.creatorCountry", "Creator Country"),
    unsupported("com.adobe.creatorWorkPhone", "Creator Phone"),
    unsupported("com.adobe.creatorWorkEmail", "Creator E-Mail"),
    unsupported("com.adobe.creatorWorkWebsite", "Creator Website"),
    unsupported("com.adobe.descriptionWriter", "Description Writer"),
    unsupported("com.adobe.iptcSubjectCode", "IPTC Subject Code"),
    unsupported("com.adobe.intellectualGenre", "Intellectual Genre"),
    unsupported("com.adobe.scene", "IPTC Scene Code"),
    def(
        "com.adobe.location",
        "Sublocation",
        Token::Field(Field::Sublocation),
        Menu::Metadata,
    ),
    unsupported("com.adobe.isoCountryCode", "ISO Country Code"),
    def(
        "com.adobe.headline",
        "Headline",
        Token::Field(Field::Headline),
        Menu::Metadata,
    ),
    unsupported("com.adobe.dateCreated", "Date Created"),
    def(
        "com.adobe.city",
        "City",
        Token::Field(Field::City),
        Menu::Metadata,
    ),
    def(
        "com.adobe.state",
        "State / Province",
        Token::Field(Field::State),
        Menu::Metadata,
    ),
    def(
        "com.adobe.country",
        "Country / Region",
        Token::Field(Field::Country),
        Menu::Metadata,
    ),
    unsupported("com.adobe.jobIdentifier", "Job Identifier"),
    unsupported("com.adobe.instructions", "Instructions"),
    unsupported("com.adobe.provider", "Credit Line"),
    unsupported("com.adobe.source", "Source"),
    unsupported("com.adobe.rightsUsageTerms", "Rights Usage Terms"),
    unsupported("com.adobe.copyrightInfoURL", "Copyright Info URL"),
    unsupported("com.adobe.altTextAccessibility", "Alt Text"),
    unsupported("com.adobe.extDescrAccessibility", "Extended Description"),
    def(
        "com.adobe.imageFileDimensions",
        "File Dimensions",
        Token::Field(Field::Dimensions),
        Menu::Exif,
    ),
    unsupported("com.adobe.imageCroppedDimensions", "Current Dimensions"),
    def(
        "com.adobe.exposure",
        "Exposure",
        Token::Field(Field::Exposure),
        Menu::Exif,
    ),
    def(
        "com.adobe.focalLength",
        "Focal Length",
        Token::Field(Field::FocalLength),
        Menu::Exif,
    ),
    def(
        "com.adobe.focalLength35mm",
        "Focal Length 35mm",
        Token::Field(Field::FocalLength35),
        Menu::Exif,
    ),
    unsupported("com.adobe.brightnessValue", "Brightness Value"),
    unsupported("com.adobe.exposureBiasValue", "Exposure Bias"),
    def(
        "com.adobe.ISOSpeedRating",
        "ISO Speed Rating",
        Token::Field(Field::Iso),
        Menu::Exif,
    ),
    unsupported("com.adobe.flash", "Flash"),
    unsupported("com.adobe.exposureProgram", "Exposure Program"),
    unsupported("com.adobe.meteringMode", "Metering Mode"),
    unsupported("com.adobe.dateTimeOriginal", "Date Time Original"),
    def(
        "com.adobe.make",
        "Make",
        Token::Field(Field::Make),
        Menu::Exif,
    ),
    def(
        "com.adobe.model",
        "Model",
        Token::Field(Field::Model),
        Menu::Exif,
    ),
    unsupported("com.adobe.serialNumber", "Serial Number"),
    def(
        "com.adobe.artist",
        "Artist",
        Token::Field(Field::Artist),
        Menu::Exif,
    ),
    def(
        "com.adobe.software",
        "Software",
        Token::Field(Field::Software),
        Menu::Exif,
    ),
    def(
        "com.adobe.lens",
        "Lens",
        Token::Field(Field::Lens),
        Menu::Exif,
    ),
    unsupported("com.adobe.GPS", "GPS"),
    unsupported("com.adobe.GPSAltitude", "Altitude"),
    unsupported("com.adobe.GPSImgDirection", "Direction"),
    def(
        "com.adobe.rating.string",
        "Rating",
        Token::Field(Field::Rating),
        Menu::Exif,
    ),
    def(
        "com.adobe.colorLabels.string",
        "Label",
        Token::Field(Field::Label),
        Menu::Exif,
    ),
    def(
        "custom_token",
        "Custom Text",
        Token::CustomText,
        Menu::Custom,
    ),
    unsupported("shoot_custom_token", "Shoot Name"),
];

/// The token Lightroom's `id` stands for. Lightroom reads its numbering
/// tokens by pattern (`naming_sequenceNumber_(%d)Digit`), so
/// "naming_sequenceNumber_3Digit" is a Sequence # (001) too.
pub(super) fn token(id: &str) -> Option<Token> {
    if let Some(def) = TOKENS.iter().find(|d| d.id == id) {
        return Some(def.token);
    }
    let digits = |prefix: &str| -> Option<u8> {
        let rest = id.strip_prefix(prefix)?;
        let mut chars = rest.chars();
        let digit = chars.next()?.to_digit(10).filter(|d| *d > 0)?;
        chars.as_str().starts_with("Digit").then_some(digit as u8)
    };
    if let Some(n) = digits("naming_sequenceNumber_") {
        return Some(Token::Sequence(n));
    }
    if let Some(n) = digits("naming_sequenceTotal_") {
        return Some(Token::Total(n));
    }
    let numbered = |prefix: &str, label: &'static str| digits(prefix).map(|_| label);
    numbered("naming_importNumber_", "Import #")
        .or_else(|| numbered("naming_operationSequence_", "Image #"))
        .map(Token::Unsupported)
}

/// The token whose menu label is `label`, any case.
pub fn by_label(label: &str) -> Option<&'static Def> {
    TOKENS.iter().find(|d| d.label.eq_ignore_ascii_case(label))
}

/// The tokens a menu offers, in order.
pub fn menu(menu: Menu) -> impl Iterator<Item = &'static Def> {
    TOKENS.iter().filter(move |d| d.menu == Some(menu))
}

/// One part of a template: literal text, or a token by its Lightroom id and
/// what it stands for (`None` when RAWmakase doesn't know the id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Token { id: String, token: Option<Token> },
}
impl Part {
    fn token(id: &str) -> Self {
        Part::Token {
            id: id.to_string(),
            token: token(id),
        }
    }
    /// The label the editor shows a known token by, when `id` is the one its
    /// menu inserts; any other id shows as itself, so it reads back as it was.
    fn label(&self) -> Option<&'static str> {
        match self {
            Part::Token { id, .. } => TOKENS.iter().find(|d| d.id == id).map(|d| d.label),
            Part::Text(_) => None,
        }
    }
}

/// A Lightroom file-name template. Its parts never have empty or adjacent
/// texts, so two templates that read the same are equal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenTemplate(Vec<Part>);

/// What filling a template needs that a photo's file may have to give.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Needs {
    /// The capture date or time.
    pub captured: bool,
    /// Lens, exposure, focal length, ISO or dimensions.
    pub info: bool,
    /// The camera's make or model.
    pub camera: bool,
    /// A title, caption, copyright or creator, which a photo the catalog
    /// has none of its own for takes from its file.
    pub descriptive: bool,
    /// What only the file says (its software, 35 mm focal length, IPTC
    /// headline and place names).
    pub facts: bool,
}
impl Needs {
    pub fn any(self) -> bool {
        self.captured || self.info || self.camera || self.descriptive || self.facts
    }
    /// What both need.
    pub fn and(self, other: Needs) -> Needs {
        Needs {
            captured: self.captured && other.captured,
            info: self.info && other.info,
            camera: self.camera && other.camera,
            descriptive: self.descriptive && other.descriptive,
            facts: self.facts && other.facts,
        }
    }
}

/// Something wrong with the editor's text, at `at` (bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayError {
    pub at: Range<usize>,
    pub message: String,
}

/// How the editor shows one stretch of its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    Text,
    /// A token RAWmakase fills.
    Token,
    /// A token it doesn't fill: unknown, or Lightroom's but not supported yet.
    Inert,
    Error,
}

/// The editor's text read: its parts with where each is, and its errors.
struct Scanned {
    parts: Vec<(Range<usize>, Part)>,
    errors: Vec<DisplayError>,
}

impl TokenTemplate {
    pub fn parts(&self) -> &[Part] {
        &self.0
    }
    fn from_parts(parts: impl IntoIterator<Item = Part>) -> Self {
        let mut out: Vec<Part> = Vec::new();
        for part in parts {
            match (out.last_mut(), part) {
                (_, Part::Text(text)) if text.is_empty() => {}
                (Some(Part::Text(last)), Part::Text(text)) => last.push_str(&text),
                (_, part) => out.push(part),
            }
        }
        Self(out)
    }

    /// Lightroom's template text `text`: `{{id}}` tokens between literal
    /// text. Any text reads; an unclosed `{{` or an empty `{{}}` is text.
    pub fn parse(text: &str) -> Self {
        let mut parts = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find("{{") {
            let Some(close) = rest[open + 2..].find("}}").map(|c| open + 2 + c) else {
                break;
            };
            let id = &rest[open + 2..close];
            if id.is_empty() {
                parts.push(Part::Text(rest[..close + 2].to_string()));
            } else {
                parts.push(Part::Text(rest[..open].to_string()));
                parts.push(Part::token(id));
            }
            rest = &rest[close + 2..];
        }
        parts.push(Part::Text(rest.to_string()));
        Self::from_parts(parts)
    }

    /// The template as Lightroom writes it.
    pub fn to_lightroom(&self) -> String {
        self.0
            .iter()
            .map(|part| match part {
                Part::Text(text) => text.clone(),
                Part::Token { id, .. } => format!("{{{{{id}}}}}"),
            })
            .collect()
    }

    /// The editor's form: tokens by their labels in braces (`{Filename}`),
    /// other ids as `{?id}`, and `{`, `}` and `\` in text (or an id) as
    /// `\{`, `\}` and `\\`.
    pub fn to_display(&self) -> String {
        self.0
            .iter()
            .map(|part| match (part, part.label()) {
                (Part::Text(text), _) => escape(text),
                (_, Some(label)) => format!("{{{label}}}"),
                (Part::Token { id, .. }, None) => format!("{{?{}}}", escape(id)),
            })
            .collect()
    }

    /// The template the editor's `text` shows, or what is wrong with it.
    pub fn from_display(text: &str) -> Result<Self, Vec<DisplayError>> {
        let scanned = scan(text);
        if !scanned.errors.is_empty() {
            return Err(scanned.errors);
        }
        let template = Self::from_parts(scanned.parts.iter().map(|(_, p)| p.clone()));
        // Lightroom's form has no escapes: text that would read back as a
        // token can't be kept.
        if Self::parse(&template.to_lightroom()) != template {
            let at = scanned
                .parts
                .iter()
                .find(|(_, p)| matches!(p, Part::Text(t) if t.contains(['{', '}'])))
                .map_or(0..text.len(), |(at, _)| at.clone());
            return Err(vec![DisplayError {
                at,
                message: "Braces here would be read as a token when the template is saved".into(),
            }]);
        }
        Ok(template)
    }

    /// How the editor shows each stretch of `text`, in order and covering it.
    pub fn spans(text: &str) -> Vec<(Range<usize>, Span)> {
        let scanned = scan(text);
        let mut spans: Vec<(Range<usize>, Span)> = scanned
            .parts
            .into_iter()
            .map(|(at, part)| {
                let span = match part {
                    Part::Text(_) => Span::Text,
                    Part::Token {
                        token: Some(Token::Unsupported(_)) | None,
                        ..
                    } => Span::Inert,
                    Part::Token { .. } => Span::Token,
                };
                (at, span)
            })
            .chain(scanned.errors.into_iter().map(|e| (e.at, Span::Error)))
            .collect();
        spans.sort_by_key(|(at, _)| at.start);
        spans
    }

    /// What it needs that a photo's file may have to give.
    pub fn needs(&self) -> Needs {
        let mut needs = Needs::default();
        for token in self.tokens() {
            match token {
                Token::Date(_) => needs.captured = true,
                Token::Field(Field::Make | Field::Model) => needs.camera = true,
                Token::Field(
                    Field::Title
                    | Field::Caption
                    | Field::Copyright
                    | Field::Creator
                    | Field::Artist,
                ) => needs.descriptive = true,
                Token::Field(
                    Field::Software
                    | Field::FocalLength35
                    | Field::Headline
                    | Field::City
                    | Field::State
                    | Field::Country
                    | Field::Sublocation,
                ) => needs.facts = true,
                Token::Field(
                    Field::Lens
                    | Field::Iso
                    | Field::Exposure
                    | Field::FocalLength
                    | Field::Dimensions,
                ) => needs.info = true,
                _ => {}
            }
        }
        needs
    }
    fn tokens(&self) -> impl Iterator<Item = Token> + '_ {
        self.0.iter().filter_map(|p| match p {
            Part::Token { token, .. } => *token,
            Part::Text(_) => None,
        })
    }
    /// Whether it has the Custom Text token.
    pub fn uses_text(&self) -> bool {
        self.tokens().any(|t| t == Token::CustomText)
    }
    /// Whether it numbers photos from Start Number.
    pub fn uses_sequence(&self) -> bool {
        self.tokens().any(|t| matches!(t, Token::Sequence(_)))
    }
    /// Whether each photo of an export gets a name of its own: with neither
    /// the file's name (current or original) nor a sequence number, photos
    /// may share one (and are then told apart by "-2", "-3"…).
    pub fn names_each_photo(&self) -> bool {
        self.tokens().any(|t| {
            matches!(
                t,
                Token::Filename | Token::OriginalFilename | Token::Sequence(_)
            )
        })
    }

    /// `source`'s new name without its extension, for its place `at`, with
    /// `text` as the Custom Text and Sequence counting from `start`; and what
    /// the name lacks. The whole name is made a valid file name once filled,
    /// so no text or value can make it name another folder.
    pub fn stem(
        &self,
        source: &Path,
        at: &NameContext,
        text: &str,
        start: u32,
    ) -> (String, Vec<String>) {
        let stem = source.file_stem().unwrap_or_default().to_string_lossy();
        let f = at.fields;
        let captured = f.captured.as_deref().and_then(Captured::parse);
        let mut notes: Vec<String> = Vec::new();
        let mut note = |note: String| {
            if !notes.contains(&note) {
                notes.push(note);
            }
        };
        let mut name = String::new();
        for part in &self.0 {
            let (id, token) = match part {
                Part::Text(text) => {
                    name.push_str(text);
                    continue;
                }
                Part::Token { id, token } => (id, token),
            };
            let Some(token) = token else {
                note(format!("unknown file name token {{{{{id}}}}}"));
                continue;
            };
            let value: Option<String> = match *token {
                Token::Filename => Some(stem.to_string()),
                Token::FilenameNumberSuffix => {
                    let digits =
                        stem.len() - stem.trim_end_matches(|c: char| c.is_ascii_digit()).len();
                    let suffix = &stem[stem.len() - digits..];
                    if suffix.is_empty() {
                        note("no number at the end of the file name".into());
                    }
                    Some(suffix.to_string())
                }
                Token::OriginalFilename => Some(match &f.original_name {
                    Some(original) => Path::new(original)
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    None => stem.to_string(),
                }),
                Token::CopyName => Some(f.copy_name.clone()),
                Token::FolderName => Some(
                    source
                        .parent()
                        .and_then(Path::file_name)
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                ),
                Token::Sequence(digits) => Some(format!(
                    "{:0width$}",
                    start as usize + at.index,
                    width = usize::from(digits)
                )),
                Token::Total(digits) => {
                    Some(format!("{:0width$}", at.total, width = usize::from(digits)))
                }
                Token::Date(part) => match captured.as_ref().and_then(|c| c.part(part)) {
                    Some(value) => Some(value),
                    None if part.time() => {
                        note("no capture time".into());
                        None
                    }
                    None => {
                        note("no capture date".into());
                        None
                    }
                },
                Token::Field(field) => {
                    let value = f.value(field);
                    if value.is_none() {
                        note(match (field, &f.camera) {
                            (Field::Make | Field::Model, super::CameraNames::NotRead) => {
                                "the camera's make and model were not read".into()
                            }
                            _ => format!("no {}", field_label(field)),
                        });
                    }
                    value
                }
                Token::CustomText => {
                    let text = text.trim();
                    // Lightroom's name for a custom text left empty.
                    Some(if text.is_empty() { "untitled" } else { text }.to_string())
                }
                Token::Unsupported(label) => {
                    note(format!("not supported yet: {label}"));
                    None
                }
            };
            name.push_str(&value.unwrap_or_default());
        }
        let mut name = name_part(&name);
        if name.is_empty() {
            note("the template gave no name: named after the file".into());
            // The file's own name, made a valid name as any other; Lightroom's
            // name for nothing at all when even that is empty.
            name = Some(name_part(&stem))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "untitled".into());
        }
        // Windows' device names can't be file names, whatever follows a dot.
        let head = name.split('.').next().unwrap_or_default();
        let name = if reserved(head) {
            format!("{head}_{}", &name[head.len()..])
        } else {
            name
        };
        (name, notes)
    }
}

fn field_label(field: Field) -> &'static str {
    TOKENS
        .iter()
        .find(|d| d.token == Token::Field(field))
        .map_or("value", |d| d.label)
}

impl super::NameFields {
    /// The value `field` names in a file name; `None` when the photo has none
    /// that is needed (an unset rating, label, title… is empty, as in
    /// Lightroom).
    fn value(&self, field: Field) -> Option<String> {
        let some = |text: &str| Some(text.trim().to_string());
        let own = |text: &Option<String>| some(text.as_deref().unwrap_or_default());
        let known =
            |text: Option<&String>| text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let info = &self.info;
        match field {
            Field::Title => own(&self.title),
            Field::Caption => own(&self.caption),
            Field::Copyright => own(&self.copyright),
            Field::Creator => own(&self.creator),
            Field::Rating => Some(if self.rating > 0 {
                self.rating.to_string()
            } else {
                String::new()
            }),
            Field::Label => some(&self.label),
            Field::Make | Field::Model => match &self.camera {
                super::CameraNames::Read { make, model } => {
                    known(if field == Field::Make { make } else { model }.as_ref())
                }
                super::CameraNames::NotRead => None,
            },
            Field::Lens => known(info.lens.as_ref()),
            Field::Iso => info.iso_text(),
            // Lightroom's "^1 at ^2", its aperture with one decimal, the
            // region's separator.
            Field::Exposure => {
                let aperture = info
                    .aperture
                    .filter(|f| *f > 0.)
                    .map(|f| format!("f / {f:.1}").replace('.', &self.decimal.0.to_string()));
                match (info.shutter_text(), aperture) {
                    (Some(s), Some(a)) => Some(format!("{s} at {a}")),
                    (s, a) => s.or(a),
                }
            }
            Field::FocalLength => info.focal_text(),
            Field::Dimensions => info
                .dimensions
                .filter(|(w, h)| *w > 0 && *h > 0)
                .map(|(w, h)| format!("{w} x {h}")),
            Field::Software => known(self.facts.software.as_ref()),
            Field::FocalLength35 => self
                .facts
                .focal_35mm
                .filter(|mm| *mm > 0)
                .map(|mm| format!("{mm} mm")),
            Field::Artist => own(&self.artist),
            Field::Keywords => some(&self.keywords),
            Field::Headline => own(&self.facts.headline),
            Field::City => own(&self.facts.city),
            Field::State => own(&self.facts.state),
            Field::Country => own(&self.facts.country),
            Field::Sublocation => own(&self.facts.sublocation),
        }
    }
}

/// A capture time read strictly: a real date, and a real time of day when
/// there is one.
struct Captured {
    year: u32,
    month: u32,
    day: u32,
    time: Option<(u32, u32, u32)>,
}
impl Captured {
    /// From the catalog's "2026-04-24 18:43:24.000" (or "T" between date
    /// and time) or EXIF's "2026:04:24 18:43:24".
    fn parse(captured: &str) -> Option<Self> {
        let number = |range: Range<usize>| -> Option<u32> {
            let text = captured.get(range)?;
            text.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| text.parse().ok())
                .flatten()
        };
        let bytes = captured.as_bytes();
        let separated =
            |at: usize, allowed: &[u8]| bytes.get(at).is_some_and(|b| allowed.contains(b));
        if !(separated(4, b"-:") && separated(7, b"-:")) {
            return None;
        }
        let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return None,
        };
        if year == 0 || day == 0 || day > days {
            return None;
        }
        let time = (separated(10, b" T") && separated(13, b":") && separated(16, b":"))
            .then(|| Some((number(11..13)?, number(14..16)?, number(17..19)?)))
            .flatten()
            .filter(|(h, m, s)| *h < 24 && *m < 60 && *s < 60);
        Some(Self {
            year,
            month,
            day,
            time,
        })
    }
    fn part(&self, part: DatePart) -> Option<String> {
        const MONTHS: [&str; 12] = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ];
        let (y, m, d) = (self.year, self.month, self.day);
        let month = MONTHS[m as usize - 1];
        Some(match part {
            DatePart::Yyyymmdd => format!("{y:04}{m:02}{d:02}"),
            DatePart::Yymmdd => format!("{:02}{m:02}{d:02}", y % 100),
            DatePart::Yyyy => format!("{y:04}"),
            DatePart::Yy => format!("{:02}", y % 100),
            DatePart::Month => month.to_string(),
            DatePart::Mon => month[..3].to_string(),
            DatePart::Mm => format!("{m:02}"),
            DatePart::Dd => format!("{d:02}"),
            DatePart::Long => format!("{month} {d:02}, {y:04}"),
            DatePart::Julian => {
                let before: u32 = (1..m)
                    .map(|month| match month {
                        4 | 6 | 9 | 11 => 30,
                        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
                        2 => 28,
                        _ => 31,
                    })
                    .sum();
                format!("{:03}", before + d)
            }
            DatePart::Hour => format!("{:02}", self.time?.0),
            DatePart::Minute => format!("{:02}", self.time?.1),
            DatePart::Second => format!("{:02}", self.time?.2),
        })
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '{' | '}' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Reads the editor's form.
fn scan(text: &str) -> Scanned {
    let mut parts = Vec::new();
    let mut errors = Vec::new();
    let mut literal = String::new();
    let mut literal_start = 0;
    let mut chars = text.char_indices().peekable();
    let flush = |literal: &mut String, start: usize, end: usize, parts: &mut Vec<_>| {
        if !literal.is_empty() {
            parts.push((start..end, Part::Text(std::mem::take(literal))));
        }
    };
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => {
                if literal.is_empty() {
                    literal_start = at;
                }
                match chars.peek() {
                    Some(&(_, next @ ('{' | '}' | '\\'))) => {
                        literal.push(next);
                        chars.next();
                    }
                    // A backslash before anything else is itself.
                    _ => literal.push('\\'),
                }
            }
            '}' => {
                flush(&mut literal, literal_start, at, &mut parts);
                errors.push(DisplayError {
                    at: at..at + 1,
                    message: "“}” without its “{”: type \\} for a brace".into(),
                });
            }
            '{' => {
                flush(&mut literal, literal_start, at, &mut parts);
                let mut inside = String::new();
                let mut end = None;
                while let Some((i, c)) = chars.next() {
                    match c {
                        '\\' => match chars.peek() {
                            Some(&(_, next @ ('{' | '}' | '\\'))) => {
                                inside.push(next);
                                chars.next();
                            }
                            _ => inside.push('\\'),
                        },
                        '}' => {
                            end = Some(i + 1);
                            break;
                        }
                        c => inside.push(c),
                    }
                }
                let Some(end) = end else {
                    errors.push(DisplayError {
                        at: at..text.len(),
                        message: "“{” without its “}”: type \\{ for a brace".into(),
                    });
                    break;
                };
                if let Some(id) = inside.strip_prefix('?') {
                    if id.is_empty() {
                        errors.push(DisplayError {
                            at: at..end,
                            message: "A token with no name".into(),
                        });
                    } else {
                        parts.push((at..end, Part::token(id)));
                    }
                } else {
                    match by_label(inside.trim()) {
                        Some(def) => parts.push((at..end, Part::token(def.id))),
                        None => errors.push(DisplayError {
                            at: at..end,
                            message: format!("No token is called “{}”", inside.trim()),
                        }),
                    }
                }
            }
            c => {
                if literal.is_empty() {
                    literal_start = at;
                }
                literal.push(c);
            }
        }
    }
    flush(&mut literal, literal_start, text.len(), &mut parts);
    Scanned { parts, errors }
}

/// A file-name template RAWmakase offers beside Lightroom's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Builtin {
    pub name: &'static str,
    /// Lightroom's form.
    pub tokens: &'static str,
}

/// RAWmakase's own templates, listed in Rename To after Lightroom's. They
/// are kept as any edited template is, so an earlier release keeps reading
/// the settings.
pub const BUILTIN: [Builtin; 3] = [
    Builtin {
        name: "Date Time - Model",
        tokens: "{{date_YYYY}}-{{date_MM}}-{{date_DD}} {{date_Hour}}.{{date_Minute}}.{{date_Second}} {{com.adobe.model}}",
    },
    Builtin {
        name: "Date Time - Filename",
        tokens: "{{date_YYYY}}-{{date_MM}}-{{date_DD}} {{date_Hour}}.{{date_Minute}}.{{date_Second}} {{image_name}}",
    },
    Builtin {
        name: "Date - Sequence",
        tokens: "{{date_YYYY}}-{{date_MM}}-{{date_DD}}-{{naming_sequenceNumber_3Digits}}",
    },
];

/// The built-in template `tokens` is, if any.
pub fn builtin(tokens: &str) -> Option<&'static Builtin> {
    BUILTIN.iter().find(|b| b.tokens == tokens)
}

#[cfg(test)]
#[path = "tokens_tests.rs"]
mod tests;
