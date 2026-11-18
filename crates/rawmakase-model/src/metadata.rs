//! Photo metadata as values: the descriptive fields RAWmakase keeps or imports
//! (title, caption, copyright, creator, capture time, location, keywords) and the
//! camera settings the Metadata panel shows. The catalog stores them, XMP and EXIF
//! read and write them, and export embeds them; this module depends on none of
//! them.
use unicode_normalization::UnicodeNormalization;

/// The default language of a language alternative.
pub const DEFAULT_LANG: &str = "x-default";

/// The text fields that have languages (XMP language alternatives).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextField {
    Title,
    /// dc:description, EXIF ImageDescription.
    Caption,
    /// dc:rights, EXIF Copyright.
    Copyright,
}
impl TextField {
    pub const ALL: [Self; 3] = [Self::Title, Self::Caption, Self::Copyright];
}

/// A field's override: what RAWmakase has instead of the file's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value<T> {
    Set(T),
    /// Empty, and the file's value left out.
    Cleared,
}

/// A text field's languages, `x-default` first, as (language, text).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LangAlt(pub Vec<(String, String)>);
impl LangAlt {
    pub fn new(text: &str) -> Self {
        Self(vec![(DEFAULT_LANG.into(), text.into())])
    }
    /// The default language's text, else the first one's.
    pub fn default_text(&self) -> Option<&str> {
        self.0
            .iter()
            .find(|(lang, _)| lang == DEFAULT_LANG)
            .or(self.0.first())
            .map(|(_, text)| text.as_str())
    }
}

/// A capture time from elsewhere than the file.
#[derive(Clone, Debug, PartialEq)]
pub struct Capture {
    /// Local time, "YYYY-MM-DDTHH:MM:SS".
    pub captured: String,
    /// The subsecond digits as read, of any length ("12", "120456").
    pub subsec: Option<String>,
    /// "+02:00", when known.
    pub offset: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Location {
    At {
        lat: f64,
        lon: f64,
        alt: Option<f64>,
    },
    /// The file's GPS left out.
    Cleared,
}

/// A photo's overrides; `None` is no row, the file's own value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Descriptive {
    pub title: Option<Value<LangAlt>>,
    pub caption: Option<Value<LangAlt>>,
    pub copyright: Option<Value<LangAlt>>,
    /// In order.
    pub creator: Option<Value<Vec<String>>>,
    pub capture: Option<Capture>,
    pub location: Option<Location>,
}
impl Descriptive {
    pub fn text(&self, field: TextField) -> Option<&Value<LangAlt>> {
        match field {
            TextField::Title => self.title.as_ref(),
            TextField::Caption => self.caption.as_ref(),
            TextField::Copyright => self.copyright.as_ref(),
        }
    }
    pub fn text_mut(&mut self, field: TextField) -> &mut Option<Value<LangAlt>> {
        match field {
            TextField::Title => &mut self.title,
            TextField::Caption => &mut self.caption,
            TextField::Copyright => &mut self.copyright,
        }
    }
}

/// A keyword and the names of its ancestors and itself, top first.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Keyword {
    pub id: i64,
    pub name: String,
    pub path: Vec<String>,
    /// Which names of `path` an export writes, by Lightroom's Include on
    /// Export and Export Containing Keywords; none when the keyword itself
    /// is not exported.
    pub exported: Vec<bool>,
}

/// A keyword name as stored and compared: NFC, case kept.
pub fn keyword_name(name: &str) -> String {
    name.trim().nfc().collect()
}

/// A keyword name as Lightroom matches a typed one: [`keyword_name`],
/// ignoring case.
pub fn keyword_key(name: &str) -> String {
    keyword_name(name).to_lowercase()
}

/// A photo's camera settings and size, as Lightroom's Metadata panel shows
/// them. Each is `None` when unknown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhotoInfo {
    pub camera: Option<String>,
    pub lens: Option<String>,
    /// Millimetres.
    pub focal: Option<f64>,
    /// The f-number.
    pub aperture: Option<f64>,
    /// Seconds.
    pub exposure: Option<f64>,
    pub iso: Option<f64>,
    /// As the photo is shown, after its orientation.
    pub dimensions: Option<(u32, u32)>,
}
impl PhotoInfo {
    /// "1/250 sec", or "2 sec" for long exposures.
    pub fn shutter_text(&self) -> Option<String> {
        let t = self.exposure.filter(|t| *t > 0.)?;
        // A fraction only where it is one: 1/250, but 0.8 sec.
        let r = 1. / t;
        Some(if t < 1. && r >= 1.5 && (r - r.round()).abs() < 0.03 * r {
            format!("1/{:.0} sec", r)
        } else {
            format!("{} sec", trim(t, 1))
        })
    }
    pub fn aperture_text(&self) -> Option<String> {
        let f = self.aperture.filter(|f| *f > 0.)?;
        Some(format!("f/{}", trim(f, 1)))
    }
    /// Lightroom's Exposure row: "1/250 sec at f/2.8".
    pub fn exposure_text(&self) -> Option<String> {
        match (self.shutter_text(), self.aperture_text()) {
            (Some(s), Some(a)) => Some(format!("{s} at {a}")),
            (s, a) => s.or(a),
        }
    }
    pub fn focal_text(&self) -> Option<String> {
        let mm = self.focal.filter(|mm| *mm > 0.)?;
        Some(format!("{} mm", trim(mm, 1)))
    }
    pub fn iso_text(&self) -> Option<String> {
        let iso = self.iso.filter(|iso| *iso > 0.)?;
        Some(format!("ISO {iso:.0}"))
    }
    pub fn dimensions_text(&self) -> Option<String> {
        let (w, h) = self.dimensions.filter(|(w, h)| *w > 0 && *h > 0)?;
        Some(format!("{w} × {h}"))
    }
}
/// `value` with at most `places` decimals and none that are zero.
fn trim(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').into()
    } else {
        text
    }
}
