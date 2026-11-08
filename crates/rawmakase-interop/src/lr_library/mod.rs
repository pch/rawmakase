//! A Lightroom library (Lightroom, not Classic: `Lightroom Library.lrlibrary`),
//! read as data. Each Adobe account's folder in it holds `Managed Catalog.mcat`,
//! SQLite used as a document store whose documents are MessagePack, and
//! `settings/<sha256>`, the Camera Raw XMP each edit names. Adobe documents
//! neither, and the library is a cache of what has synced to this computer, so
//! everything here is read leniently: a field that isn't there or isn't what
//! it should be leaves its part out, and the caller counts a document that
//! can't be read rather than failing on it.
mod msgpack;
#[cfg(test)]
mod tests;

use anyhow::{Context, Result, ensure};
use msgpack::Value;
use std::path::{Path, PathBuf};

/// The largest document read, far beyond any Lightroom writes.
const MAX_DOCUMENT: usize = 16 << 20;

/// The catalog in each account's folder of a library.
pub const CATALOG: &str = "Managed Catalog.mcat";

/// A photo of the library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asset {
    pub id: String,
    /// The original's name and size when it was added to Lightroom.
    pub file_name: String,
    pub file_size: u64,
    /// The SHA-256 of the original file, in lowercase hex; empty when unknown.
    pub sha256: String,
    /// The latest rating given, 0 to 5.
    pub rating: i32,
    /// The latest review: 1 picked, −1 rejected, 0 neither.
    pub flag: i32,
    /// The SHA-256 naming the edit's Camera Raw XMP in `settings/`.
    pub edit: Option<String>,
    /// When it was taken, as Lightroom keeps it: `2023-09-08T14:03:11`,
    /// without a zone. Empty when unknown.
    pub capture_date: String,
}

/// An album, or a folder of albums (`set`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub set: bool,
}

/// A photo of an album.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlbumAsset {
    pub album: String,
    pub asset: String,
}

/// What a document of the library is, as far as an import needs it.
#[derive(Clone, Debug, PartialEq)]
pub enum Document {
    Asset(Asset),
    Album(Album),
    AlbumAsset(AlbumAsset),
}

/// The account folders of `library` that hold a catalog, in sorted order.
pub fn accounts(library: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(library)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(CATALOG).is_file())
        .collect();
    found.sort();
    found
}

/// The Camera Raw XMP of edit `sha256` in `account`, the folder holding the
/// catalog that names it. None when the name isn't a SHA-256, so a document
/// can't point outside `settings/`.
pub fn edit_path(account: &Path, sha256: &str) -> Option<PathBuf> {
    is_sha256(sha256).then(|| account.join("settings").join(sha256))
}

/// Where Lightroom keeps `asset`'s original in `account` once it has it:
/// `originals/<year>/<year>-<month>-<day>/<file name>`, by its capture date.
/// None when it has no capture date or its name isn't a plain file name, so
/// a document can't point outside `originals/`.
pub fn original_path(account: &Path, asset: &Asset) -> Option<PathBuf> {
    let day = asset.capture_date.get(..10)?;
    let b = day.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    let dated = digits(0..4) && b[4] == b'-' && digits(5..7) && b[7] == b'-' && digits(8..10);
    let name = asset.file_name.as_str();
    let plain =
        !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0']);
    (dated && &day[..4] != "0000" && plain).then(|| {
        account
            .join("originals")
            .join(&day[..4])
            .join(day)
            .join(name)
    })
}

/// Whether a document's annotation, the part of it Lightroom keeps on this
/// computer only, marks it to be kept here: Store Album Locally, which
/// Lightroom sets on the album and on each of its photos.
pub fn marked_for_download(annotation: &[u8]) -> bool {
    msgpack::decode(annotation).is_ok_and(|v| {
        v.get(LOCAL_ONLY)
            .and_then(|l| l.get(MARKED))
            .is_some_and(|m| *m == Value::Bool(true))
    })
}

/// The annotation `annotation` (none when the document has none) marked as
/// [`marked_for_download`] asks, every other byte of it as it was; None when
/// it is marked already. An error when it isn't a MessagePack map.
pub fn mark_for_download(annotation: Option<&[u8]>) -> Result<Option<Vec<u8>>> {
    ensure!(
        annotation.is_none_or(|a| a.len() <= MAX_DOCUMENT),
        "Annotation too large"
    );
    msgpack::set_true(annotation.unwrap_or(&[0x80]), LOCAL_ONLY, MARKED)
}

/// A new album's document, as Lightroom writes one when an album is made on
/// this computer: `now` is the time it is made, as Lightroom writes times
/// (`2026-11-08T10:22:05.786Z`).
pub fn new_album(name: &str, now: &str) -> Vec<u8> {
    msgpack::encode(&entries(vec![
        ("userUpdated", Value::Str(now.into())),
        ("type", Value::Str("album".into())),
        ("name", Value::Str(name.into())),
        ("subtype", Value::Str("collection".into())),
        ("userCreated", Value::Str(now.into())),
    ]))
}

/// The document putting photo `asset` in album `album`, as Lightroom writes
/// one.
pub fn new_album_asset(album: &str, asset: &str, now: &str) -> Vec<u8> {
    let id = |id: &str| entries(vec![("id", Value::Str(id.into()))]);
    msgpack::encode(&entries(vec![
        ("album", id(album)),
        ("type", Value::Str("album_asset".into())),
        ("userUpdated", Value::Str(now.into())),
        ("asset", id(asset)),
        ("userCreated", Value::Str(now.into())),
    ]))
}

/// A new album's annotation: when it was made here, and that Lightroom is
/// to keep its photos here (Store Album Locally), as Lightroom writes it.
pub fn new_album_annotation(now: &str) -> Vec<u8> {
    msgpack::encode(&entries(vec![(
        LOCAL_ONLY,
        entries(vec![
            ("updateTimeStamp", Value::Str(now.into())),
            (MARKED, Value::Bool(true)),
        ]),
    )]))
}

/// A new `album_asset` document's annotation: Lightroom writes an empty
/// array.
pub const NEW_ALBUM_ASSET_ANNOTATION: &[u8] = &[0x90];

fn entries(pairs: Vec<(&str, Value)>) -> Value {
    Value::Map(
        pairs
            .into_iter()
            .map(|(k, v)| (Value::Str(k.into()), v))
            .collect(),
    )
}

/// The map of an annotation Lightroom keeps for this computer alone, and the
/// key in it Store Album Locally sets.
const LOCAL_ONLY: &str = "_localOnly";
const MARKED: &str = "isMarkedForOfflineUse";

/// The document of `type_`/`subtype` with id `id` and MessagePack `content`,
/// if it is one an import uses; None for the other kinds. An error when its
/// content isn't MessagePack.
pub fn document(type_: &str, subtype: &str, id: &str, content: &[u8]) -> Result<Option<Document>> {
    let wanted = matches!(
        (type_, subtype),
        ("asset", "image") | ("album", "collection" | "collection_set") | ("album_asset", _)
    );
    if !wanted {
        return Ok(None);
    }
    // Lightroom's documents are kilobytes; a value takes more memory than
    // its bytes, so a damaged one of hundreds of megabytes isn't decoded.
    ensure!(content.len() <= MAX_DOCUMENT, "Document too large");
    let v = msgpack::decode(content).context("Not MessagePack")?;
    let id_of = |v: &Value, key| {
        v.get(key)
            .and_then(|x| x.get("id"))
            .and_then(Value::as_str)
            .map(String::from)
    };
    Ok(match type_ {
        "asset" => Some(Document::Asset(asset(id, &v))),
        "album" => Some(Document::Album(Album {
            id: id.into(),
            name: text(v.get("name")),
            parent: id_of(&v, "parent"),
            set: subtype == "collection_set",
        })),
        _ => match (id_of(&v, "album"), id_of(&v, "asset")) {
            (Some(album), Some(asset)) => Some(Document::AlbumAsset(AlbumAsset { album, asset })),
            _ => None,
        },
    })
}

fn asset(id: &str, v: &Value) -> Asset {
    let source = v.get("importSource");
    let sha256 = text(source.and_then(|s| s.get("sha256"))).to_ascii_lowercase();
    let edit = v
        .get("develop")
        .and_then(|d| d.get("xmpCameraRaw"))
        .and_then(|x| x.get("sha256"))
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase)
        .filter(|s| is_sha256(s));
    let rating = latest(v.get("ratings"), "rating")
        .and_then(Value::as_i64)
        .map_or(0, |r| r.clamp(0, 5) as i32);
    let flag = match latest(v.get("reviews"), "flag").and_then(Value::as_str) {
        Some("pick") => 1,
        Some("reject") => -1,
        _ => 0,
    };
    Asset {
        id: id.into(),
        file_name: text(source.and_then(|s| s.get("fileName"))),
        file_size: source
            .and_then(|s| s.get("fileSize"))
            .and_then(Value::as_i64)
            .and_then(|n| u64::try_from(n).ok())
            .unwrap_or(0),
        sha256: if is_sha256(&sha256) {
            sha256
        } else {
            String::new()
        },
        rating,
        flag,
        edit,
        capture_date: text(v.get("captureDate")),
    }
}

/// Of a map from user to `{date, <field>}`, as ratings and reviews are kept,
/// the field given last. Dates are ISO 8601 in UTC, so they sort as text.
fn latest<'a>(by_user: Option<&'a Value>, field: &str) -> Option<&'a Value> {
    by_user?
        .entries()?
        .iter()
        .filter_map(|(_, given)| {
            Some((
                given.get("date").and_then(Value::as_str).unwrap_or(""),
                given.get(field)?,
            ))
        })
        .max_by(|a, b| a.0.cmp(b.0))
        .map(|(_, value)| value)
}

fn text(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or_default().to_string()
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
