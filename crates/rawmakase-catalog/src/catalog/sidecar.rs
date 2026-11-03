//! Descriptive metadata read from XMP sidecars and from the XMP inside JPEG,
//! TIFF, PNG, WebP, AVIF, HEIF, JPEG XL, DNG and CR3 files (a CR3's found by the
//! reader the app passes in): when a folder is added, for its
//! new photos, and on Read Metadata from Files, for photos already in the
//! catalog. Never at render time; the catalog stays the source of truth.
use super::db::{LightroomWrite, Write, sql, sqlite_sql};
use super::value::{TextOrBlob, row};
use super::{Catalog, PhotoId};
use crate::jpeg::{APP1, Segments};
use crate::storage::RasterFormat;
use crate::xml::ns::JPEG_HEADER;
use crate::xmp::descriptive::{self, Read};
use crate::{isobmff, riff};
use anyhow::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// What reading sidecars found besides the metadata.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidecarReport {
    /// Files that could not be read, with the reason.
    pub unreadable: Vec<(PathBuf, String)>,
    /// A second sidecar of a RAW, unused because the first one wins.
    pub ignored: Vec<PathBuf>,
}
impl SidecarReport {
    pub(super) fn add(&mut self, other: SidecarReport) {
        self.unreadable.extend(other.unreadable);
        self.ignored.extend(other.ignored);
    }
    /// "2 sidecars could not be read", or nothing to say.
    pub fn summary(&self) -> Option<String> {
        let plural = |n: usize| if n == 1 { "sidecar" } else { "sidecars" };
        let mut parts = Vec::new();
        let n = self.unreadable.len();
        if n > 0 {
            parts.push(format!("{n} {} could not be read", plural(n)));
        }
        let n = self.ignored.len();
        if n > 0 {
            parts.push(format!("{n} {} ignored", plural(n)));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
    /// Each unreadable file and why, and each ignored sidecar, one per line.
    pub fn details(&self) -> String {
        self.unreadable
            .iter()
            .map(|(path, why)| format!("{}: {why}", path.display()))
            .chain(
                self.ignored
                    .iter()
                    .map(|p| format!("{}: ignored, another sidecar is used", p.display())),
            )
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The suffix a sidecar's name adds, in the case lookup tries first.
const SUFFIX: &str = ".xmp";

/// `base` with [`SUFFIX`] added, in lower or upper case, if it exists.
fn existing(base: &Path) -> Option<PathBuf> {
    [SUFFIX.to_ascii_lowercase(), SUFFIX.to_ascii_uppercase()]
        .into_iter()
        .map(|s| {
            let mut name = base.as_os_str().to_owned();
            name.push(s);
            PathBuf::from(name)
        })
        .find(|p| p.is_file())
}

/// What the sidecars `file` could have are named after, in the order lookup
/// tries them; each takes [`SUFFIX`]. A RAW's are digiKam's
/// "IMG_1234.NEF.xmp", then "IMG_1234.xmp"; a JPEG, TIFF or PNG has only
/// "IMG_1234.JPG.xmp", as "IMG_1234.xmp" beside a RAW of the same name is the
/// RAW's. The one rule of which file reads which sidecar.
pub(super) fn sidecar_candidates(file: &Path) -> Vec<PathBuf> {
    let mut bases = vec![file.to_path_buf()];
    if crate::storage::is_raw(file) {
        bases.push(file.with_extension(""));
    }
    bases.dedup();
    bases
}

/// `name` compared as loosely as a volume may compare names: in Unicode
/// compatibility form (NFKC) and case-folded through lower and upper case,
/// so `ẞ`, `ß` and `SS` are `ss` and `ﬁ` is `fi`. Meant to be at least as
/// coarse as APFS's and NTFS's own comparisons, so it may take two names
/// for one that no volume does: what refusals compare (whether a name is
/// taken, whether another file reads a sidecar), never what picks a file
/// to move.
pub(super) fn caseless(name: &OsStr) -> String {
    use unicode_normalization::UnicodeNormalization;
    let text: String = name.to_string_lossy().nfkc().collect();
    // Lower first, so `ẞ` becomes `ß` and then, through upper case, `ss`.
    text.to_lowercase()
        .to_uppercase()
        .to_lowercase()
        .nfkc()
        .collect()
}

/// The sidecar names `file` reads by [`sidecar_candidates`], exactly.
pub(super) fn sidecar_names(file: &Path) -> Vec<OsString> {
    sidecar_candidates(file)
        .into_iter()
        .map(|base| {
            let mut name = base.into_os_string();
            name.push(SUFFIX);
            name
        })
        .collect()
}

/// The sidecar names `file` reads by [`sidecar_candidates`], as
/// [`caseless`] compares them.
pub(super) fn sidecar_names_caseless(file: &Path) -> Vec<String> {
    sidecar_candidates(file)
        .into_iter()
        .map(|base| {
            let mut name = base.into_os_string();
            name.push(SUFFIX);
            caseless(&name)
        })
        .collect()
}

/// The sidecars of `file`, the one used first (see [`sidecar_candidates`]).
pub fn sidecars(file: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = sidecar_candidates(file)
        .iter()
        .filter_map(|base| existing(base))
        .collect();
    found.dedup();
    found
}

/// Reads the XMP a camera embeds in a RAW the catalog can't read it from
/// itself (a CR3's, found by RAWmakase's RAW decoders), so the app passes
/// its reader in: `photo::raw_xmp`. `None` for other files.
pub type RawXmpReader = fn(&Path) -> Option<Vec<u8>>;

/// The XMP inside a JPEG (its standard APP1 packet), TIFF (tag 700), PNG (its
/// XMP text chunk), WebP ("XMP " chunk), AVIF or HEIF (its XMP item) or JPEG
/// XL ("xml " box, or "brob" box compressing one); in another file, what
/// `raw_xmp` finds.
fn embedded(file: &Path, raw_xmp: RawXmpReader) -> Result<Option<String>> {
    match RasterFormat::of(file) {
        Some(RasterFormat::Jpeg) => {
            use std::io::Read as _;
            // Segment by segment up to the image data, not the whole file.
            let Some(mut segments) = Segments::new(std::fs::File::open(file)?)? else {
                return Ok(None);
            };
            while let Some(segment) = segments.next_segment()? {
                if segment.marker == APP1 && segment.length > JPEG_HEADER.len() {
                    let mut data = vec![0u8; segment.length];
                    segments.reader().read_exact(&mut data)?;
                    if let Some(xmp) = data.strip_prefix(JPEG_HEADER) {
                        return Ok(Some(
                            String::from_utf8(xmp.to_vec())
                                .context("embedded XMP is not valid UTF-8")?,
                        ));
                    }
                }
            }
            Ok(None)
        }
        Some(RasterFormat::Tiff) => in_tiff(file),
        Some(RasterFormat::Png) => in_png(file),
        Some(RasterFormat::Webp) => in_webp(file),
        Some(RasterFormat::Avif | RasterFormat::Heif) => in_heif(file),
        Some(RasterFormat::Jxl) => in_jxl(file),
        // A DNG keeps its XMP in the same TIFF tag.
        None if file
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dng")) =>
        {
            in_tiff(file)
        }
        None => text(raw_xmp(file)),
    }
}

/// The XMP in a WebP's "XMP " chunk.
fn in_webp(file: &Path) -> Result<Option<String>> {
    let mut f = std::fs::File::open(file)?;
    let Some(chunk) =
        riff::webp_chunks(&mut f).and_then(|c| c.into_iter().find(|c| &c.kind == b"XMP "))
    else {
        return Ok(None);
    };
    text(riff::chunk_data(&mut f, &chunk))
}

/// The XMP item ("application/rdf+xml") describing an AVIF's or a HEIF's
/// primary image.
fn in_heif(file: &Path) -> Result<Option<String>> {
    let mut f = std::fs::File::open(file)?;
    let Some(heif) = isobmff::Heif::read(&mut f) else {
        return Ok(None);
    };
    let xmp = heif
        .describing(*b"cdsc", heif.primary)
        .into_iter()
        .find(|id| {
            heif.item(*id)
                .is_some_and(|i| &i.kind == b"mime" && i.content_type == "application/rdf+xml")
        });
    match xmp {
        // No larger than a WebP chunk or a JPEG XL box read for its XMP.
        Some(id) => text(heif.data_up_to(&mut f, id, 16 << 20)),
        None => Ok(None),
    }
}

/// The XMP in a JPEG XL container's "xml " box, brotli-compressed ("brob")
/// or not.
fn in_jxl(file: &Path) -> Result<Option<String>> {
    let mut f = std::fs::File::open(file)?;
    text(isobmff::jxl_metadata(&mut f, *b"xml "))
}

/// Embedded XMP bytes as text; malformed containers (`None`) have none.
fn text(bytes: Option<Vec<u8>>) -> Result<Option<String>> {
    bytes
        .map(|b| String::from_utf8(b).context("embedded XMP is not valid UTF-8"))
        .transpose()
}

/// The XMP in a PNG's "XML:com.adobe.xmp" text chunk, uncompressed as the
/// XMP standard has it, before or after the image data.
fn in_png(file: &Path) -> Result<Option<String>> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    const KEYWORD: &[u8] = b"XML:com.adobe.xmp\0";
    let mut f = std::fs::File::open(file)?;
    let mut at = 8u64;
    loop {
        f.seek(SeekFrom::Start(at))?;
        let mut header = [0u8; 8];
        if f.read_exact(&mut header).is_err() {
            return Ok(None);
        }
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
        match &header[4..] {
            b"iTXt" if length <= 16_000_000 => {
                let mut data = vec![0u8; length as usize];
                f.read_exact(&mut data)?;
                // Keyword, then compression flag and method, language and
                // translated keyword, each ended by a NUL, then the text.
                if let Some([0, _, rest @ ..]) = data.strip_prefix(KEYWORD) {
                    let mut parts = rest.splitn(3, |b| *b == 0);
                    if let (Some(_), Some(_), Some(text)) =
                        (parts.next(), parts.next(), parts.next())
                    {
                        return Ok(Some(
                            String::from_utf8(text.to_vec())
                                .context("embedded XMP is not valid UTF-8")?,
                        ));
                    }
                }
            }
            b"IEND" => return Ok(None),
            _ => {}
        }
        at += 12 + length;
    }
}

/// The XMP in a TIFF's tag 700.
fn in_tiff(file: &Path) -> Result<Option<String>> {
    let Some(mut t) = crate::tiff::Tiff::open(std::fs::File::open(file)?, 0) else {
        return Ok(None);
    };
    let Some(main) = t.ifd(t.first) else {
        return Ok(None);
    };
    main.get(&crate::exif::tag::XMP)
        .and_then(|e| t.raw(e))
        .map(String::from_utf8)
        .transpose()
        .context("embedded XMP is not valid UTF-8")
}

/// A sidecar's text: UTF-8, or UTF-16 by its byte order mark, as XML allows.
fn decode(bytes: &[u8]) -> Result<String> {
    let utf16 = |big: bool| -> Result<String> {
        let (pairs, rest) = bytes[2..].as_chunks::<2>();
        anyhow::ensure!(rest.is_empty(), "UTF-16 text cut short");
        let units: Vec<u16> = pairs
            .iter()
            .map(|c| {
                if big {
                    u16::from_be_bytes(*c)
                } else {
                    u16::from_le_bytes(*c)
                }
            })
            .collect();
        Ok(String::from_utf16(&units)?)
    };
    match bytes {
        [0xfe, 0xff, ..] => utf16(true),
        [0xff, 0xfe, ..] => utf16(false),
        _ => Ok(
            std::str::from_utf8(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes))?.to_string(),
        ),
    }
}

/// The metadata of `file`'s sidecar and its own XMP (see [`embedded`]; a
/// RAW's as `raw_xmp` reads it): field by field the sidecar's where it has
/// one, an explicitly empty value included, else the file's. `None` when
/// neither has any.
pub fn read_file(file: &Path, raw_xmp: RawXmpReader) -> (Option<Read>, SidecarReport) {
    let mut report = SidecarReport::default();
    let mut found = sidecars(file).into_iter();
    let first = found.next();
    report.ignored.extend(found);
    let sidecar = first.and_then(|path| {
        // Larger than any sidecar: not read into memory.
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        match (if size > 16_000_000 {
            Err(anyhow::anyhow!("too large for a sidecar"))
        } else {
            std::fs::read(&path).context("not readable")
        })
        .and_then(|bytes| descriptive::read(&decode(&bytes)?))
        {
            Ok(read) => Some(read),
            Err(e) => {
                report.unreadable.push((path, format!("{e:#}")));
                None
            }
        }
    });
    let inside =
        match embedded(file, raw_xmp).and_then(|x| x.map(|t| descriptive::read(&t)).transpose()) {
            Ok(read) => read,
            Err(e) => {
                report
                    .unreadable
                    .push((file.to_path_buf(), format!("{e:#}")));
                None
            }
        };
    let read = match (sidecar, inside) {
        (Some(s), Some(i)) => Some(s.or(i)),
        (s, i) => s.or(i),
    };
    (read, report)
}

/// How metadata read from files meets the catalog's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    /// Only fields a photo has no value for; keywords are added.
    FillEmpty,
    /// Every field a file has; keywords replace the photo's.
    Overwrite,
}

/// Writes what was read into photo `id`'s rows, in `w`'s transaction, as
/// `merge` says. Rating, label and flag are set where the file has them.
pub(super) fn apply(w: &mut Write<'_>, id: PhotoId, read: &Read, merge: Merge) -> Result<()> {
    let overwrite = merge == Merge::Overwrite;
    let mut d = super::descriptive::read(w, id)?;
    fn put<T: Clone>(slot: &mut Option<T>, value: &Option<T>, overwrite: bool) {
        if value.is_some() && (overwrite || slot.is_none()) {
            slot.clone_from(value);
        }
    }
    put(&mut d.title, &read.title, overwrite);
    put(&mut d.caption, &read.caption, overwrite);
    put(&mut d.copyright, &read.copyright, overwrite);
    put(&mut d.creator, &read.creator, overwrite);
    put(&mut d.capture, &read.capture, overwrite);
    put(&mut d.location, &read.location, overwrite);
    super::descriptive::write(w, id, &d)?;
    if let Some(paths) = &read.keywords {
        if overwrite {
            w.execute(sql!("DELETE FROM photo_keywords WHERE photo=?"), &[&id])?;
        }
        for path in paths {
            let keyword = super::descriptive::keyword_at(w, path)?;
            super::descriptive::tag_photo(w, id, keyword)?;
        }
    }
    if let Some(rating) = read.rating {
        w.execute(
            sql!("UPDATE photos SET rating=? WHERE id=?"),
            &[&rating, &id],
        )?;
    }
    if let Some(label) = &read.label {
        w.execute(sql!("UPDATE photos SET label=? WHERE id=?"), &[label, &id])?;
    }
    if let Some(flag) = read.flag {
        w.execute(sql!("UPDATE photos SET flag=? WHERE id=?"), &[&flag, &id])?;
    }
    Ok(())
}

impl Catalog {
    /// Lightroom's Read Metadata from Files, for the masters of `ids`: every
    /// field a photo's sidecar or embedded XMP has replaces the catalog's,
    /// edits included; fields the files lack are left alone. One
    /// transaction. Virtual copies are never read.
    #[cfg(test)]
    pub fn read_metadata_from_files(&mut self, ids: &[PhotoId]) -> Result<SidecarReport> {
        let mut report = SidecarReport::default();
        let reads: Vec<(PhotoId, PathBuf, Read)> = self
            .photos()?
            .into_iter()
            .filter(|p| ids.contains(&p.id) && p.master.is_none())
            .filter_map(|p| {
                let (read, found) = read_file(&p.path, |_| None);
                report.add(found);
                read.map(|r| (p.id, p.path, r))
            })
            .collect();
        report.add(self.apply_file_metadata(&reads, Merge::Overwrite)?);
        Ok(report)
    }
    /// Writes what was read from files, in one transaction, as `merge`
    /// says. A photo whose values can't be written (an impossible date) is
    /// left as it was and reported with its file.
    pub fn apply_file_metadata(
        &mut self,
        reads: &[(PhotoId, PathBuf, Read)],
        merge: Merge,
    ) -> Result<SidecarReport> {
        self.db.write(|w| {
            let mut report = SidecarReport::default();
            for (id, path, read) in reads {
                // One photo that can't be written leaves the others saved.
                if let Err(e) = w.savepoint(|w| apply(w, *id, read, merge)) {
                    report.unreadable.push((path.clone(), format!("{e:#}")));
                }
            }
            Ok(report)
        })
    }
}

/// Title, caption, creator, copyright, capture time and location from the XMP
/// a Lightroom catalog keeps per photo (attached as `lr`), for the photos
/// that have no row yet. Keywords, rating, label and flag come from
/// Lightroom's own tables. Returns the photos read.
pub(super) fn copy_lightroom_metadata(lr: &mut LightroomWrite<'_>) -> Result<usize> {
    if !lr.has_table("Adobe_AdditionalMetadata")? {
        return Ok(0);
    }
    row! {
        struct Packet {
            id: PhotoId,
            xmp: TextOrBlob,
        }
    }
    let rows: Vec<Packet> = lr.read_sqlite(
        sqlite_sql!(concat!(
            // Only photos that are still that Lightroom image: a photo
            // added since may have taken a removed copy's id.
            "SELECT m.image, m.xmp FROM lr.Adobe_AdditionalMetadata m
             JOIN photos p ON p.id = m.image
             JOIN lr.Adobe_images i ON i.id_local = m.image
             JOIN lr.AgLibraryFile f ON f.id_local = i.rootFile
             JOIN lr.AgLibraryFolder d ON d.id_local = f.folder
             JOIN lr.AgLibraryRootFolder r ON r.id_local = d.rootFolder
             WHERE p.original_path = r.absolutePath || d.pathFromRoot || ",
            super::lightroom::lightroom_filename!()
        )),
        &[],
    )?;
    let mut read = 0;
    for Packet { id, xmp } in rows {
        // Invalid text is left out rather than imported mangled.
        let xmp = xmp
            .0
            .and_then(|t| String::from_utf8(t).ok())
            .unwrap_or_default();
        // A packet that can't be read is left out, as Lightroom's own data.
        let Ok(found) = descriptive::read(&xmp) else {
            continue;
        };
        let descriptive_only = Read {
            keywords: None,
            rating: None,
            label: None,
            flag: None,
            ..found
        };
        apply(lr.write(), id, &descriptive_only, Merge::FillEmpty)?;
        read += 1;
    }
    Ok(read)
}

/// Set in `meta` once descriptive metadata has been copied from the stored
/// Lightroom catalog.
pub(super) const METADATA_BACKFILLED: &str = "lightroom_metadata_backfilled";

impl Catalog {
    /// Catalogs imported before descriptive metadata was kept still hold
    /// the original Lightroom catalog; copy it once, for photos with no row.
    pub fn backfill_lightroom_metadata(&mut self) -> Result<usize> {
        self.backfill_once(METADATA_BACKFILLED, copy_lightroom_metadata)
    }
}

#[cfg(test)]
#[path = "sidecar_tests.rs"]
mod tests;
