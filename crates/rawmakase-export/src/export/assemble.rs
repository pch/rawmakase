//! Which metadata an export carries, decided in one place from the Include
//! choice and each field's value: the photo's own in the catalog (set or
//! cleared), else the file's EXIF. Lightroom's groups: Copyright (dc:rights,
//! EXIF Copyright), Contact (creator), Descriptive (title, caption, keywords,
//! rating, label), capture time, location, Camera (make, model, exposure,
//! lens) and Camera Raw (the develop settings).
use crate::exif::{CameraExif, Field, MAX_VALUE, tag::*};
use crate::export_settings::{ExportSettings, Include};
use crate::metadata::{Capture, Descriptive, LangAlt, Location, Value};
use crate::tiff::kind::{BYTE, RATIONAL};
use crate::xmp::write::KeywordPath;

/// Lightroom's caption.
const CAPTION: u16 = IMAGE_DESCRIPTION;
/// Dates of the EXIF directory: original, digitized, their offsets and
/// subseconds. They go with the capture time, not the camera.
const DATES: [u16; 8] = [
    DATE_TIME_ORIGINAL,
    DATE_TIME_DIGITIZED,
    OFFSET_TIME,
    OFFSET_TIME_ORIGINAL,
    OFFSET_TIME_DIGITIZED,
    SUBSEC_TIME,
    SUBSEC_TIME_ORIGINAL,
    SUBSEC_TIME_DIGITIZED,
];

/// The groups an export includes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Policy {
    /// Custom: the four switches of earlier releases, which this keeps
    /// exactly. The catalog's descriptive fields go with `descriptive`; the
    /// camera's own caption, artist and copyright with the camera info.
    pub custom: bool,
    pub copyright: bool,
    pub contact: bool,
    pub descriptive: bool,
    pub capture_time: bool,
    pub camera: bool,
    pub location: bool,
    pub develop: bool,
}
impl Policy {
    pub fn of(s: &ExportSettings) -> Self {
        let all = |camera, develop| Self {
            custom: false,
            copyright: true,
            contact: true,
            descriptive: true,
            capture_time: true,
            camera,
            location: !s.remove_location,
            develop,
        };
        let copyright = |contact| Self {
            custom: false,
            copyright: true,
            contact,
            descriptive: false,
            capture_time: false,
            camera: false,
            location: false,
            develop: false,
        };
        match s.include {
            Include::CopyrightOnly => copyright(false),
            Include::CopyrightAndContact => copyright(true),
            Include::AllExceptCameraAndCameraRaw => all(false, false),
            Include::AllExceptCameraRaw => all(true, false),
            Include::All => all(true, true),
            Include::Custom => Self {
                custom: true,
                copyright: s.descriptive,
                contact: s.descriptive,
                descriptive: s.descriptive,
                capture_time: s.capture,
                camera: s.capture,
                location: s.capture && s.location,
                develop: s.develop,
            },
        }
    }
    /// Whether the file's EXIF is needed: for the camera info in Custom, as
    /// before, and for any field the catalog does not hold otherwise.
    pub fn reads_file(&self) -> bool {
        if self.custom { self.camera } else { true }
    }
    /// Whether an XMP packet is written at all: in Custom, as before, only
    /// for develop settings or descriptive metadata.
    pub fn writes_xmp(&self) -> bool {
        !self.custom || self.develop || self.descriptive
    }
}

/// The photo's catalog values that export can carry.
#[derive(Clone, Debug, Default)]
pub struct Values {
    pub descriptive: Descriptive,
    pub keywords: Vec<KeywordPath>,
    pub rating: i32,
    pub label: String,
}

/// What an export writes besides pixels, before its own size and software.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Assembled {
    pub exif: CameraExif,
    /// XMP fields; `None` when no packet is written.
    pub xmp: Option<XmpFields>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XmpFields {
    pub captured: Option<String>,
    pub created: Option<String>,
    pub rating: i32,
    pub label: String,
    pub keywords: Vec<KeywordPath>,
    pub title: Vec<(String, String)>,
    pub caption: Vec<(String, String)>,
    pub rights: Vec<(String, String)>,
    pub creators: Vec<String>,
    pub lens: bool,
    pub develop: bool,
}

/// A field's value for export: the catalog's when set, nothing when
/// cleared, else the file's when `file` allows it.
fn resolve(value: &Option<Value<LangAlt>>, file: Option<String>) -> Vec<(String, String)> {
    match value {
        Some(Value::Set(langs)) => langs.0.clone(),
        Some(Value::Cleared) => Vec::new(),
        None => file
            .filter(|t| !t.is_empty())
            .map(|t| LangAlt::new(&t).0)
            .unwrap_or_default(),
    }
}

/// The photo's copyright for the Simple Copyright Watermark: the catalog's,
/// else the file's; `None` when cleared or empty.
pub fn copyright(values: &Values, file: Option<&CameraExif>) -> Option<String> {
    match &values.descriptive.copyright {
        Some(Value::Set(langs)) => langs.default_text().map(str::to_string),
        Some(Value::Cleared) => None,
        None => file.and_then(|e| e.get(COPYRIGHT).and_then(Field::text)),
    }
    .filter(|c| !c.is_empty())
}

/// Decides every tag of an export. `file` is the source's EXIF when it could
/// be read; `libraw`, the decoder's capture settings (and a RAW container's
/// own dates), stands in for the camera info and dates when it could not.
pub fn assemble(
    policy: Policy,
    file: Option<&CameraExif>,
    libraw: Option<CameraExif>,
    values: &Values,
) -> Assembled {
    let d = &values.descriptive;
    let text = |tag| file.and_then(|f| f.get(tag)).and_then(Field::text);
    let camera = file.cloned().or(libraw);
    let mut exif = CameraExif::default();
    if policy.camera
        && let Some(camera) = &camera
    {
        exif.main = camera
            .main
            .iter()
            .filter(|f| ![CAPTION, ARTIST, COPYRIGHT].contains(&f.tag))
            .cloned()
            .collect();
        exif.exif = camera
            .exif
            .iter()
            .filter(|f| !DATES.contains(&f.tag))
            .cloned()
            .collect();
    }
    if policy.capture_time {
        if let Some(camera) = &camera {
            exif.exif.extend(
                camera
                    .exif
                    .iter()
                    .filter(|f| DATES.contains(&f.tag))
                    .cloned(),
            );
        }
        if let Some(c) = &d.capture {
            capture_tags(&mut exif.exif, c);
        }
    }
    if policy.location {
        exif.gps = match &d.location {
            // The location the catalog read from this very GPS directory
            // (Add Folder, Read Metadata from Files): the directory whole, so
            // its time, direction and other tags go with it.
            Some(location) if file.is_some_and(|f| f.location().as_ref() == Some(location)) => {
                file.map(|f| f.gps.clone()).unwrap_or_default()
            }
            Some(Location::At { lat, lon, alt }) => gps(*lat, *lon, *alt),
            Some(Location::Cleared) => Vec::new(),
            None => file.map(|f| f.gps.clone()).unwrap_or_default(),
        };
    }
    // Caption, creator and copyright: the catalog's value where the group is
    // included, else (Custom) the camera's own with the camera info; a
    // cleared field never goes out.
    let creators: Vec<String> = match &d.creator {
        Some(Value::Set(names)) => names.clone(),
        Some(Value::Cleared) => Vec::new(),
        None => text(ARTIST).filter(|t| !t.is_empty()).into_iter().collect(),
    };
    let field = |tag| file.and_then(|f| f.get(tag)).cloned();
    for (tag, group, value) in [
        (CAPTION, policy.descriptive, &d.caption),
        (COPYRIGHT, policy.copyright, &d.copyright),
    ] {
        let set = match value {
            Some(Value::Set(langs)) => Some(langs.default_text().unwrap_or_default().to_string()),
            _ => None,
        };
        exif.main.extend(main_tag(
            policy,
            tag,
            group,
            value == &Some(Value::Cleared),
            set,
            field(tag),
        ));
    }
    let artist = match &d.creator {
        Some(Value::Set(names)) => Some(names.join("; ")),
        _ => None,
    };
    exif.main.extend(main_tag(
        policy,
        ARTIST,
        policy.contact,
        d.creator == Some(Value::Cleared),
        artist,
        field(ARTIST),
    ));
    exif.main.sort_by_key(|f| f.tag);
    exif.exif.sort_by_key(|f| f.tag);
    let xmp = policy.writes_xmp().then(|| {
        // Custom wrote no catalog field it did not hold, and never the
        // camera's own in XMP.
        let file_text = |tag| if policy.custom { None } else { text(tag) };
        let pick = |group: bool, langs: Vec<(String, String)>| {
            if group { langs } else { Vec::new() }
        };
        XmpFields {
            captured: policy
                .capture_time
                .then(|| camera.as_ref().and_then(CameraExif::captured))
                .flatten(),
            created: policy
                .capture_time
                .then(|| d.capture.as_ref().map(xmp_time))
                .flatten(),
            rating: if policy.descriptive { values.rating } else { 0 },
            label: if policy.descriptive {
                values.label.clone()
            } else {
                String::new()
            },
            keywords: if policy.descriptive {
                values.keywords.clone()
            } else {
                Vec::new()
            },
            title: pick(policy.descriptive, resolve(&d.title, None)),
            caption: pick(policy.descriptive, resolve(&d.caption, file_text(CAPTION))),
            rights: pick(
                policy.copyright,
                resolve(&d.copyright, file_text(COPYRIGHT)),
            ),
            creators: if policy.contact && !(policy.custom && d.creator.is_none()) {
                creators.clone()
            } else {
                Vec::new()
            },
            lens: policy.custom || policy.camera,
            develop: policy.develop,
        }
    });
    Assembled { exif, xmp }
}

/// Caption, artist or copyright in the main directory: the catalog's value
/// where its group is included, else the file's own as it is; a cleared
/// field never goes out. Custom keeps earlier releases' rule: the camera's
/// own go with the camera info, the catalog's in their place when
/// descriptive metadata is on.
fn main_tag(
    policy: Policy,
    tag: u16,
    group: bool,
    cleared: bool,
    set: Option<String>,
    file: Option<Field>,
) -> Option<Field> {
    if cleared {
        return None;
    }
    let set = set.filter(|v| !v.is_empty());
    // Text this long goes in the XMP only: the EXIF must fit one JPEG
    // segment with everything else. Never the file's in its place.
    let catalog = || match &set {
        Some(v) if v.len() > MAX_VALUE => None,
        Some(v) => Some(Field::ascii(tag, v)),
        None => file.clone(),
    };
    match (policy.custom, policy.camera, group) {
        (true, false, _) => None,
        (true, true, true) | (false, _, true) => catalog(),
        (true, true, false) => file,
        (false, _, false) => None,
    }
}

/// DateTimeOriginal, SubSecTimeOriginal and OffsetTimeOriginal from a
/// capture time kept in the catalog; the camera's are replaced, its offset
/// dropped when the catalog has none.
fn capture_tags(exif: &mut Vec<Field>, c: &Capture) {
    exif.retain(|f| {
        ![
            DATE_TIME_ORIGINAL,
            SUBSEC_TIME_ORIGINAL,
            OFFSET_TIME_ORIGINAL,
        ]
        .contains(&f.tag)
    });
    let (date, time) = c.captured.split_once('T').unwrap_or((&c.captured, ""));
    exif.push(Field::ascii(
        DATE_TIME_ORIGINAL,
        &format!("{} {time}", date.replace('-', ":")),
    ));
    if let Some(subsec) = c.subsec.as_deref().filter(|s| !s.is_empty()) {
        exif.push(Field::ascii(SUBSEC_TIME_ORIGINAL, subsec));
    }
    if let Some(offset) = c.offset.as_deref().filter(|s| !s.is_empty()) {
        exif.push(Field::ascii(OFFSET_TIME_ORIGINAL, offset));
    }
}

/// xmp:CreateDate's form: "2024-05-01T12:30:15.12+02:00".
fn xmp_time(c: &Capture) -> String {
    let mut out = c.captured.clone();
    if let Some(subsec) = c.subsec.as_deref().filter(|s| !s.is_empty()) {
        out.push('.');
        out.push_str(subsec);
    }
    if let Some(offset) = &c.offset {
        out.push_str(offset);
    }
    out
}

/// A GPS directory for a position: version, latitude, longitude and
/// altitude, each as degrees, minutes and seconds.
fn gps(lat: f64, lon: f64, alt: Option<f64>) -> Vec<Field> {
    let dms = |tag, value: f64| {
        let value = value.abs();
        let degrees = value.trunc();
        let minutes = ((value - degrees) * 60.).trunc();
        let seconds = ((value - degrees) * 60. - minutes) * 60.;
        let mut bytes = Vec::new();
        for (n, d) in [
            (degrees as u32, 1u32),
            (minutes as u32, 1),
            ((seconds * 10_000.).round() as u32, 10_000),
        ] {
            bytes.extend(n.to_le_bytes());
            bytes.extend(d.to_le_bytes());
        }
        Field {
            tag,
            kind: RATIONAL,
            count: 3,
            bytes,
        }
    };
    let reference = |tag, letter: &str| Field::ascii(tag, letter);
    let mut fields = vec![
        Field {
            tag: GPS_VERSION_ID,
            kind: BYTE,
            count: 4,
            bytes: vec![2, 3, 0, 0],
        },
        reference(GPS_LATITUDE_REF, if lat < 0. { "S" } else { "N" }),
        dms(GPS_LATITUDE, lat),
        reference(GPS_LONGITUDE_REF, if lon < 0. { "W" } else { "E" }),
        dms(GPS_LONGITUDE, lon),
    ];
    if let Some(alt) = alt {
        fields.push(Field {
            tag: GPS_ALTITUDE_REF,
            kind: BYTE,
            count: 1,
            bytes: vec![u8::from(alt < 0.)],
        });
        fields.push(Field::rational(
            GPS_ALTITUDE,
            (alt.abs() * 100.).round() as u32,
            100,
        ));
    }
    fields
}

#[cfg(test)]
mod tests;
