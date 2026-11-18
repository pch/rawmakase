//! The Metadata filter's attributes, as Lightroom's Library Filter › Metadata
//! lists them, and each entry's values of them. Camera, lens and the shooting
//! settings come from photo info, as [`Facets`]; file type and keywords from
//! the entry itself.
use super::Entry;
use crate::ids::PhotoId;
use crate::metadata::PhotoInfo;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

/// What a column of the Metadata filter lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Attribute {
    Camera,
    Lens,
    FocalLength,
    Aperture,
    ShutterSpeed,
    Iso,
    FileType,
    /// Each of a photo's keywords: a photo counts under every one it has.
    Keyword,
}
impl Attribute {
    /// The columns the filter shows, in order.
    pub const ALL: [Self; 8] = [
        Self::Camera,
        Self::Lens,
        Self::FocalLength,
        Self::Aperture,
        Self::ShutterSpeed,
        Self::Iso,
        Self::FileType,
        Self::Keyword,
    ];
    /// The attribute's key in the saved layout, kept when its name changes.
    pub fn key(self) -> &'static str {
        match self {
            Self::Camera => "camera",
            Self::Lens => "lens",
            Self::FocalLength => "focal_length",
            Self::Aperture => "aperture",
            Self::ShutterSpeed => "shutter_speed",
            Self::Iso => "iso",
            Self::FileType => "file_type",
            Self::Keyword => "keyword",
        }
    }
    /// The attribute saved as `key`; None for one this version doesn't know.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == key)
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Camera => "Camera",
            Self::Lens => "Lens",
            Self::FocalLength => "Focal Length",
            Self::Aperture => "Aperture",
            Self::ShutterSpeed => "Shutter Speed",
            Self::Iso => "ISO Speed",
            Self::FileType => "File Type",
            Self::Keyword => "Keyword",
        }
    }
    /// What a photo without this attribute is listed as.
    pub fn unknown(self) -> &'static str {
        match self {
            Self::Camera => "Unknown Camera",
            Self::Lens => "Unknown Lens",
            Self::FocalLength => "Unknown Focal Length",
            Self::Aperture => "Unknown Aperture",
            Self::ShutterSpeed => "Unknown Shutter Speed",
            Self::Iso => "Unknown ISO",
            Self::FileType => "Unknown File Type",
            Self::Keyword => "No Keyword",
        }
    }
    /// Where [`Facets`] keeps this attribute; None for one read from the
    /// entry itself.
    fn slot(self) -> Option<usize> {
        match self {
            Self::Camera => Some(0),
            Self::Lens => Some(1),
            Self::FocalLength => Some(2),
            Self::Aperture => Some(3),
            Self::ShutterSpeed => Some(4),
            Self::Iso => Some(5),
            Self::FileType | Self::Keyword => None,
        }
    }
    /// Whether the values come from photo info, so [`Facets`] are needed.
    pub fn needs_info(self) -> bool {
        self.slot().is_some()
    }
    /// `entry`'s values of this attribute, none when it has none.
    pub fn values<'a>(self, entry: &Entry<'a>) -> Vec<Cow<'a, str>> {
        match self.slot() {
            Some(slot) => entry
                .facets
                .and_then(|f| f.0[slot].as_deref())
                .map(Cow::Borrowed)
                .into_iter()
                .collect(),
            None if self == Self::FileType => file_type(entry.filename).into_iter().collect(),
            None => keywords(entry.keywords).map(Cow::Borrowed).collect(),
        }
    }
}

/// A photo's photo-info values, as the Metadata panel shows them ("23 mm",
/// "f/2.8", "1/250 sec", "ISO 400"), so photos the panel shows alike are
/// counted together; None where the info has none.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facets([Option<String>; 6]);
impl Facets {
    pub fn of(info: &PhotoInfo) -> Self {
        let name = |t: &Option<String>| {
            t.as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
        };
        Self([
            name(&info.camera),
            name(&info.lens),
            info.focal_text(),
            info.aperture_text(),
            info.shutter_text(),
            info.iso_text(),
        ])
    }
}

/// Every photo's facets by id, as the catalog's photo info reads them.
pub type PhotoFacets = HashMap<PhotoId, Facets>;

/// The file type as Lightroom's File Type column names it: Raw, DNG, JPEG,
/// TIFF, HEIF, else the extension in capitals.
fn file_type(filename: &str) -> Option<Cow<'static, str>> {
    let (_, extension) = filename.rsplit_once('.')?;
    let is = |names: &[&str]| names.iter().any(|n| extension.eq_ignore_ascii_case(n));
    Some(Cow::Borrowed(if is(&["dng"]) {
        "DNG"
    } else if crate::storage::is_raw(Path::new(filename)) {
        "Raw"
    } else if is(&["jpg", "jpeg"]) {
        "JPEG"
    } else if is(&["tif", "tiff"]) {
        "TIFF"
    } else if is(&["heic", "heif", "hif"]) {
        "HEIF"
    } else if extension.is_empty() {
        return None;
    } else {
        return Some(Cow::Owned(extension.to_ascii_uppercase()));
    }))
}

/// The keyword names, each whole.
fn keywords(keywords: &[String]) -> impl Iterator<Item = &str> {
    keywords.iter().map(|k| k.trim()).filter(|k| !k.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photos_shown_alike_count_together() {
        let info = |focal, aperture| PhotoInfo {
            camera: Some(" X-T5 ".into()),
            lens: Some(String::new()),
            focal: Some(focal),
            aperture: Some(aperture),
            ..PhotoInfo::default()
        };
        let a = Facets::of(&info(23.04, 2.0));
        let b = Facets::of(&info(23., 1.99));
        assert_eq!(a, b);
        let entry = Entry {
            facets: Some(&a),
            ..Entry::default()
        };
        let values = |attribute: Attribute| attribute.values(&entry);
        assert_eq!(values(Attribute::Camera), ["X-T5"]);
        assert!(values(Attribute::Lens).is_empty());
        assert_eq!(values(Attribute::FocalLength), ["23 mm"]);
        assert!(values(Attribute::Iso).is_empty());
    }
    #[test]
    fn file_types_read_as_in_lightroom() {
        for (filename, kind) in [
            ("a.RAF", Some("Raw")),
            ("a.dng", Some("DNG")),
            ("a.jpeg", Some("JPEG")),
            ("a.TIF", Some("TIFF")),
            ("a.png", Some("PNG")),
            ("a", None),
        ] {
            let entry = Entry {
                filename,
                ..Entry::default()
            };
            let values = Attribute::FileType.values(&entry);
            assert_eq!(values.first().map(|v| v.as_ref()), kind, "{filename}");
        }
    }
}
