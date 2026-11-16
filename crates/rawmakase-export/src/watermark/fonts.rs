//! Fonts for text watermarks: Inter, which RAWmakase ships, at its weights,
//! and the families installed on the computer with the styles each has.
use anyhow::{Context, Result, bail};
use skrifa::{FontRef, MetadataProvider, string::StringId};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// The default family, shipped with RAWmakase.
pub const INTER: &str = "Inter";
/// Inter 4.001 as a variable font: the file fastframe-fonts bundles for the app's
/// interface (the app's tests check they are the same bytes), under the SIL Open
/// Font License in `licenses/Inter-OFL.txt`.
pub const INTER_DATA: &[u8] = include_bytes!("../../fonts/InterVariable.ttf");
/// Inter's styles, by weight.
const INTER_FACES: [(&str, f32); 4] = [
    ("Regular", 400.),
    ("Medium", 500.),
    ("SemiBold", 600.),
    ("Bold", 700.),
];

/// A face of an installed family.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub name: String,
    path: PathBuf,
    index: u32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Family {
    pub name: String,
    pub faces: Vec<Face>,
}

/// A loaded face: its data, index in a collection, and weight for a
/// variable one.
#[derive(Clone)]
pub struct Font {
    pub(super) data: Arc<Vec<u8>>,
    pub(super) index: u32,
    pub(super) weight: Option<f32>,
}
impl Font {
    pub(super) fn face(&self) -> Result<FontRef<'_>> {
        FontRef::from_index(&self.data, self.index).context("Font not readable")
    }
}

/// Inter, with its styles.
pub fn inter() -> Family {
    Family {
        name: INTER.into(),
        faces: INTER_FACES
            .iter()
            .map(|(name, _)| Face {
                name: (*name).into(),
                path: PathBuf::new(),
                index: 0,
            })
            .collect(),
    }
}

static FAMILIES: OnceLock<Vec<Family>> = OnceLock::new();

/// The families once listed, for an interface that must not wait for the
/// listing; `None` until then.
pub fn families_if_listed() -> Option<&'static [Family]> {
    FAMILIES.get().map(Vec::as_slice)
}

/// Every family: Inter first, then the installed ones by name. Installed
/// fonts are listed once, on first use; reading their names only touches
/// each file's header and name table.
pub fn families() -> &'static [Family] {
    FAMILIES.get_or_init(|| {
        let mut families = vec![inter()];
        families.extend(installed());
        families
    })
}

fn installed() -> Vec<Family> {
    let mut files = Vec::new();
    fn walk(dir: &std::path::Path, depth: usize, files: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                if depth < 4 {
                    walk(&path, depth + 1, files);
                }
            } else if path.extension().is_some_and(|e| {
                matches!(
                    e.to_string_lossy().to_ascii_lowercase().as_str(),
                    "ttf" | "otf" | "ttc" | "otc"
                )
            }) {
                files.push(path);
            }
        }
    }
    for dir in super::font_dirs::font_directories() {
        walk(&dir, 0, &mut files);
    }
    // Directories in their probing order, files by name within each: the
    // same face wins on every run when two files claim it.
    files.dedup();
    let mut families: std::collections::BTreeMap<String, Vec<Face>> = Default::default();
    for path in files {
        // Mapped, so only the header and name table are read.
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        // SAFETY: read only while this loop holds it, assuming, as
        // fastframe-fonts does, that installed fonts aren't truncated while
        // they are being listed.
        let Ok(data) = (unsafe { memmap2::Mmap::map(&file) }) else {
            continue;
        };
        for index in 0..face_count(&data) {
            let Ok(font) = FontRef::from_index(&data, index) else {
                continue;
            };
            // Fonts that can't draw outlines (bitmap emoji) or Latin text
            // make poor watermarks.
            let Some(a) = font.charmap().map('A') else {
                continue;
            };
            if font.outline_glyphs().get(a).is_none() {
                continue;
            }
            let name = |ids: [StringId; 2]| {
                ids.into_iter().find_map(|id| {
                    font.localized_strings(id)
                        .english_or_first()
                        .map(|s| s.to_string())
                        .filter(|s| !s.trim().is_empty())
                })
            };
            let (Some(family), Some(face)) = (
                name([StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME]),
                name([
                    StringId::TYPOGRAPHIC_SUBFAMILY_NAME,
                    StringId::SUBFAMILY_NAME,
                ]),
            ) else {
                continue;
            };
            // Hidden system faces (".SF NS") are not offered.
            if family.starts_with('.') || family == INTER {
                continue;
            }
            let faces = families.entry(family).or_default();
            if !faces.iter().any(|f| f.name == face) {
                faces.push(Face {
                    name: face,
                    path: path.clone(),
                    index,
                });
            }
        }
    }
    families
        .into_iter()
        .map(|(name, faces)| Family { name, faces })
        .collect()
}

fn face_count(data: &[u8]) -> u32 {
    if data.starts_with(b"ttcf") && data.len() >= 12 {
        u32::from_be_bytes([data[8], data[9], data[10], data[11]]).min(64)
    } else {
        1
    }
}

/// Loads `face` of `family`; Inter is built in.
pub fn load(family: &str, face: &str) -> Result<Font> {
    if family == INTER {
        let weight = INTER_FACES
            .iter()
            .find(|(name, _)| *name == face)
            .map_or(400., |(_, w)| *w);
        // Copied once and shared by every load.
        static DATA: OnceLock<Arc<Vec<u8>>> = OnceLock::new();
        return Ok(Font {
            data: DATA.get_or_init(|| Arc::new(INTER_DATA.to_vec())).clone(),
            index: 0,
            weight: Some(weight),
        });
    }
    let Some(found) = families()
        .iter()
        .find(|f| f.name == family)
        .and_then(|f| f.faces.iter().find(|x| x.name == face))
    else {
        bail!("Watermark font not found: {family} {face}");
    };
    let data = std::fs::read(&found.path)
        .with_context(|| format!("Watermark font not readable: {family} {face}"))?;
    Ok(Font {
        data: Arc::new(data),
        index: found.index,
        weight: None,
    })
}
