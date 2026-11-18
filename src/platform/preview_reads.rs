//! How much of a RAW is read where its sensor data isn't wanted.
//!
//! Library thumbnails and the photo-info backfill want a RAW's embedded
//! preview or its header, and read those in pieces on every volume
//! ([`in_pieces`]): reading the file whole instead took the file's size in
//! memory for each (twice, until `raw::own` kept the buffer it was read into) and,
//! on a 100,000-photo catalog of 43 MB files, 3.2 TB of reads to fill in the
//! photo info. A file not read that way (an IIQ, a header without exposure) is
//! read whole, as before.
//!
//! Develop's placeholder is different: it decodes the photo next and needs the
//! whole file anyway, so it reads the embedded preview alone first only on
//! network volumes (SMB, NFS, WebDAV, FUSE; see
//! `rawmakase_fs::VolumeFacts::local`), where the whole file takes long to
//! arrive ([`previews_only`]).
//!
//! `RAWMAKASE_PREVIEW_READS` overrides both: `always` (the placeholder's
//! preview read alone on every volume), `never` (every file read whole), or
//! `auto` (the default), e.g. to compare the two on one volume.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Auto,
    Always,
    Never,
}

fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| {
        match std::env::var("RAWMAKASE_PREVIEW_READS")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "always" => Mode::Always,
            "never" => Mode::Never,
            _ => Mode::Auto,
        }
    })
}

/// Whether a RAW's header or embedded preview is read in pieces, without its
/// sensor data, where nothing else of the file is wanted: always, unless
/// `RAWMAKASE_PREVIEW_READS=never`.
pub(crate) fn in_pieces() -> bool {
    mode() != Mode::Never
}

/// Whether Develop shows the RAW at `path` from its embedded preview read
/// alone, before the whole file it decodes next. Asks the file system once
/// per folder; that can wait on a stalled network mount, so call it off the
/// UI thread.
pub(crate) fn previews_only(path: &Path) -> bool {
    match mode() {
        Mode::Always => true,
        Mode::Never => false,
        Mode::Auto => !local(path.parent().unwrap_or(path)),
    }
}

/// Whether `folder` is on a local volume, as last found; a folder whose volume
/// can't be asked counts as local, so it is read as before.
fn local(folder: &Path) -> bool {
    static KNOWN: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Default::default);
    if let Some(local) = known
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(folder)
    {
        return *local;
    }
    let local = rawmakase_fs::volume_facts(folder).map_or(true, |f| f.local);
    known
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(folder.to_path_buf(), local);
    local
}

/// A RAW with a header and an embedded preview that are read in pieces, for
/// tests of the callers that want only those.
#[cfg(test)]
pub(crate) mod fixture {
    use std::path::{Path, PathBuf};

    /// The preview's size.
    pub(crate) const PREVIEW: [u32; 2] = [720, 480];

    /// A synthetic chart DNG in `dir` (tests/corpus/charts) that a camera could
    /// have written: with an exposure (1/250 s at f/2.8, ISO 200) in an EXIF
    /// directory and a JPEG preview in a second directory, both added after
    /// its sensor data, and its first directory written again with links to
    /// them.
    pub(crate) fn dng(dir: &Path) -> PathBuf {
        let chart =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/synthetic-d65.dng");
        let mut out = std::fs::read(chart).unwrap();
        assert_eq!(&out[..4], b"II*\0", "a little-endian TIFF");
        let u32_at =
            |out: &[u8], at: usize| u32::from_le_bytes(out[at..at + 4].try_into().unwrap());
        let first = u32_at(&out, 4) as usize;
        let count = u16::from_le_bytes(out[first..first + 2].try_into().unwrap()) as usize;
        let mut entries: Vec<[u8; 12]> = (0..count)
            .map(|i| {
                out[first + 2 + 12 * i..first + 14 + 12 * i]
                    .try_into()
                    .unwrap()
            })
            .collect();
        // One directory entry: a tag, its type (3 a short, 4 a long, 5 a
        // rational at an offset), one value.
        let entry = |tag: u16, kind: u16, value: u32| -> [u8; 12] {
            let mut e = [0; 12];
            e[..2].copy_from_slice(&tag.to_le_bytes());
            e[2..4].copy_from_slice(&kind.to_le_bytes());
            e[4..8].copy_from_slice(&1u32.to_le_bytes());
            e[8..].copy_from_slice(&value.to_le_bytes());
            e
        };
        let directory = |out: &mut Vec<u8>, entries: &[[u8; 12]], next: u32| -> u32 {
            out.resize(out.len() + out.len() % 2, 0);
            let at = out.len() as u32;
            out.extend((entries.len() as u16).to_le_bytes());
            out.extend(entries.iter().flatten());
            out.extend(next.to_le_bytes());
            at
        };
        let mut jpeg = Vec::new();
        let preview = image::RgbImage::from_fn(PREVIEW[0], PREVIEW[1], |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&preview)
            .unwrap();
        out.resize(out.len() + out.len() % 2, 0);
        let jpeg_at = out.len() as u32;
        out.extend(&jpeg);
        out.resize(out.len() + out.len() % 2, 0);
        let rationals = out.len() as u32;
        for value in [1u32, 250, 28, 10] {
            out.extend(value.to_le_bytes());
        }
        // ExposureTime, FNumber and ISOSpeedRatings.
        let exif = directory(
            &mut out,
            &[
                entry(33434, 5, rationals),
                entry(33437, 5, rationals + 8),
                entry(34855, 3, 200),
            ],
            0,
        );
        // NewSubFileType (a reduced image), JPEGInterchangeFormat and its length.
        let second = directory(
            &mut out,
            &[
                entry(254, 4, 1),
                entry(513, 4, jpeg_at),
                entry(514, 4, jpeg.len() as u32),
            ],
            0,
        );
        entries.push(entry(34665, 4, exif));
        entries.sort_by_key(|e| u16::from_le_bytes([e[0], e[1]]));
        let first = directory(&mut out, &entries, second);
        out[4..8].copy_from_slice(&first.to_le_bytes());
        let path = dir.join("camera.dng");
        std::fs::write(&path, out).unwrap();
        path
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_local_folder_is_read_as_before() {
        let dir = tempfile::tempdir().unwrap();
        assert!(super::local(dir.path()));
        // Asked once, then known.
        assert!(super::local(dir.path()));
    }

    /// Thumbnails and photo info are read in pieces on every volume, a local
    /// one too; Develop's placeholder only where the volume isn't local.
    #[test]
    fn only_develop_s_placeholder_depends_on_the_volume() {
        let dir = tempfile::tempdir().unwrap();
        assert!(super::in_pieces());
        assert!(!super::previews_only(&dir.path().join("photo.dng")));
    }

    /// The fixture is a RAW both ways of reading agree on.
    #[test]
    fn the_fixture_reads_the_same_in_pieces_and_whole() -> anyhow::Result<()> {
        use crate::metadata::PhotoInfo;
        let dir = tempfile::tempdir()?;
        let path = super::fixture::dng(dir.path());
        let whole = crate::photo::open(&path)?.metadata;
        let header = crate::photo::header_metadata(&path)?;
        assert_eq!(
            PhotoInfo::from_metadata(&header),
            PhotoInfo::from_metadata(&whole)
        );
        assert_eq!((whole.iso, whole.aperture), (200., 2.8));
        let pieces = crate::raw::thumbnail_reaching_from_file(&path, 640)?;
        let mut raw = crate::raw::Raw::open_file(&path)?;
        assert_eq!(pieces, crate::raw::thumbnail_reaching(&mut raw, 640)?);
        assert_eq!(pieces.dimensions(), super::fixture::PREVIEW.into());
        Ok(())
    }
}
