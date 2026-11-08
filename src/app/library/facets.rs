//! The Metadata filter's data, as Lightroom's Library Filter › Metadata
//! shows it: the attributes a column can list, each photo's values of them,
//! and how many photos have each value. Camera, lens and the shooting settings
//! come from the catalog's photo info, read for every photo at once when the
//! filter or the search first needs them, and again after a reload or once
//! info read from files is saved. File type and keywords come from the photo.
use super::Library;
use super::sort::natural;
use crate::catalog::{Photo, PhotoId};
use crate::metadata::PhotoInfo;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;

/// What a column of the Metadata filter lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Attribute {
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
    pub(super) const ALL: [Self; 8] = [
        Self::Camera,
        Self::Lens,
        Self::FocalLength,
        Self::Aperture,
        Self::ShutterSpeed,
        Self::Iso,
        Self::FileType,
        Self::Keyword,
    ];
    pub(super) fn name(self) -> &'static str {
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
    pub(super) fn unknown(self) -> &'static str {
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
    /// photo itself.
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
    pub(super) fn needs_info(self) -> bool {
        self.slot().is_some()
    }
    /// `p`'s values of this attribute, none when it has none; `facets` are
    /// its photo info's.
    pub(super) fn values<'a>(self, p: &'a Photo, facets: Option<&'a Facets>) -> Vec<Cow<'a, str>> {
        match self.slot() {
            Some(slot) => facets
                .and_then(|f| f.0[slot].as_deref())
                .map(Cow::Borrowed)
                .into_iter()
                .collect(),
            None if self == Self::FileType => file_type(&p.filename).into_iter().collect(),
            None => keywords(p).map(Cow::Borrowed).collect(),
        }
    }
    /// How two values of this attribute sort: by duration for shutter
    /// speeds (1/4000 sec before 1/250 sec), else as people read them
    /// (23 mm before 105 mm).
    fn compare(self, a: &str, b: &str) -> Ordering {
        if self == Self::ShutterSpeed
            && let (Some(x), Some(y)) = (seconds(a), seconds(b))
        {
            return x.total_cmp(&y);
        }
        natural(a, b).then_with(|| a.cmp(b))
    }
}

/// A photo's photo-info values, as the Metadata panel shows them ("23 mm",
/// "f/2.8", "1/250 sec", "ISO 400"), so photos the panel shows alike are
/// counted together; None where the info has none.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Facets([Option<String>; 6]);
impl Facets {
    pub(super) fn of(info: &PhotoInfo) -> Self {
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

/// Every photo's facets by id, as [`crate::catalog::Catalog::photo_infos`]
/// reads them.
pub(super) type PhotoFacets = HashMap<PhotoId, Facets>;

/// `p`'s facets: its own, else its master's, for a virtual copy made since
/// they were read.
pub(super) fn facets_of<'a>(facets: &'a PhotoFacets, p: &Photo) -> Option<&'a Facets> {
    facets
        .get(&p.id)
        .or_else(|| p.master.and_then(|m| facets.get(&m)))
}

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

/// `p`'s keywords. The catalog joins their names with ", ", and Lightroom
/// keeps commas out of keyword names.
fn keywords(p: &Photo) -> impl Iterator<Item = &str> {
    p.keywords
        .split(", ")
        .map(str::trim)
        .filter(|k| !k.is_empty())
}

/// A shutter speed as [`PhotoInfo::shutter_text`] writes it, in seconds.
fn seconds(text: &str) -> Option<f64> {
    let number = text.strip_suffix(" sec")?;
    match number.split_once('/') {
        Some((top, bottom)) => Some(top.parse::<f64>().ok()? / bottom.parse::<f64>().ok()?),
        None => number.parse().ok(),
    }
}

/// One column's values with their photo counts.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Counts {
    /// The photos counted, each once, whatever its values: All's count.
    pub total: usize,
    /// Each value and its photos, in the order the attribute sorts, then
    /// the photos without one.
    pub values: Vec<(Option<String>, usize)>,
}
impl Counts {
    pub(super) fn sorted(
        attribute: Attribute,
        total: usize,
        counts: HashMap<Option<String>, usize>,
    ) -> Self {
        let mut values: Vec<_> = counts.into_iter().collect();
        values.sort_by(|(a, _), (b, _)| match (a, b) {
            (Some(a), Some(b)) => attribute.compare(a, b),
            (a, b) => a.is_none().cmp(&b.is_none()),
        });
        Self { total, values }
    }
}

/// The Library's copy of every photo's facets and the counts last shown.
#[derive(Default)]
pub(super) struct Cache {
    facets: Option<PhotoFacets>,
    /// Photo info was saved since the facets were read: they are read again
    /// once the background read ends, not after each of its batches, as each
    /// read is of the whole catalog.
    stale: bool,
    /// Each column's counts, for the photos shown at that `shown_version`,
    /// while the Metadata menu shows them.
    counts: Option<(u64, Vec<Counts>)>,
}
impl Cache {
    /// `attribute`'s counts as last worked out by
    /// [`Library::update_metadata_counts`].
    pub(super) fn counts(&self, attribute: Attribute) -> Option<&Counts> {
        let at = Attribute::ALL.iter().position(|a| *a == attribute)?;
        self.counts.as_ref().map(|(_, counts)| &counts[at])
    }
}

impl Library {
    /// Reads every photo's facets from the catalog, unless they are read
    /// already. A catalog that can't be read gives none, with a warning: the
    /// filter then counts every photo as unknown until the catalog is read
    /// again or more photo info is saved.
    pub(super) fn load_facets(&mut self) {
        if self.facets.facets.is_some() {
            return;
        }
        let infos = self.session.catalog().photo_infos().unwrap_or_else(|e| {
            self.messages.warn(format!(
                "Photo info could not be read for the Metadata filter: {e}"
            ));
            HashMap::new()
        });
        let facets = infos
            .iter()
            .map(|(id, info)| (*id, Facets::of(info)))
            .collect();
        self.facets.facets = Some(facets);
        self.facets.counts = None;
    }
    /// Forgets the facets read, after the catalog was read again.
    pub(super) fn forget_facets(&mut self) {
        self.facets = Cache::default();
    }
    /// After photo info was saved (`saved`) or the background read ended:
    /// whether the facets were forgotten, to be read again, which they are
    /// once the read ends.
    pub(super) fn facets_after_info_saved(&mut self, saved: bool) -> bool {
        self.facets.stale |= saved && self.facets.facets.is_some();
        if self.facets.stale && !self.session.reading_photo_info() {
            self.forget_facets();
            return true;
        }
        false
    }
    pub(super) fn facets(&self) -> Option<&PhotoFacets> {
        self.facets.facets.as_ref()
    }
    /// Works out how many photos have each value of each attribute, among
    /// those the source and the rest of the filter bar show: as the bar would
    /// be with that column's choice cleared, so its other values stay there to
    /// add. Done again only once the photos shown change.
    pub(super) fn update_metadata_counts(&mut self) {
        self.load_facets();
        let version = self.shown_version;
        if self
            .facets
            .counts
            .as_ref()
            .is_none_or(|(at, _)| *at != version)
        {
            let facets = self.facets.facets.as_ref();
            let counts = Attribute::ALL
                .map(|a| {
                    self.filters.counts(
                        self.session.photos(),
                        |path| self.availability.is_available(path),
                        facets,
                        a,
                    )
                })
                .to_vec();
            self.facets.counts = Some((version, counts));
        }
    }
    #[cfg(test)]
    pub(super) fn metadata_counts(&mut self, attribute: Attribute) -> Vec<(Option<String>, usize)> {
        self.update_metadata_counts();
        self.facets.counts(attribute).unwrap().values.clone()
    }
    #[cfg(test)]
    pub(super) fn metadata_total(&mut self, attribute: Attribute) -> usize {
        self.update_metadata_counts();
        self.facets.counts(attribute).unwrap().total
    }
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
        let photo = Photo::default();
        let values = |attribute: Attribute| attribute.values(&photo, Some(&a));
        assert_eq!(values(Attribute::Camera), ["X-T5"]);
        assert!(values(Attribute::Lens).is_empty());
        assert_eq!(values(Attribute::FocalLength), ["23 mm"]);
        assert!(values(Attribute::Iso).is_empty());
    }
    #[test]
    fn file_types_and_shutter_speeds_read_as_in_lightroom() {
        for (filename, kind) in [
            ("a.RAF", Some("Raw")),
            ("a.dng", Some("DNG")),
            ("a.jpeg", Some("JPEG")),
            ("a.TIF", Some("TIFF")),
            ("a.png", Some("PNG")),
            ("a", None),
        ] {
            let photo = Photo {
                filename: filename.into(),
                ..Photo::default()
            };
            let values = Attribute::FileType.values(&photo, None);
            assert_eq!(values.first().map(|v| v.as_ref()), kind, "{filename}");
        }
        let mut speeds: Vec<&str> = vec!["2 sec", "1/250 sec", "0.8 sec", "1/4000 sec"];
        speeds.sort_by(|a, b| Attribute::ShutterSpeed.compare(a, b));
        assert_eq!(speeds, ["1/4000 sec", "1/250 sec", "0.8 sec", "2 sec"]);
    }
}
