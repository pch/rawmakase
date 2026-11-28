//! Fonts for text watermarks: the system default at its weights,
//! and the families installed on the computer with the styles each has.
use anyhow::{Context, Result, bail};
use skrifa::{FontRef, MetadataProvider, string::StringId};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// The family name of the default watermark font, resolved from the fonts
/// installed on this computer.
pub const SYSTEM: &str = "System";
const SYSTEM_FACES: [(&str, f32); 4] = [
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

/// The system default, with its styles.
pub fn system() -> Family {
    Family {
        name: SYSTEM.into(),
        faces: SYSTEM_FACES
            .iter()
            .map(|(name, _)| Face {
                name: (*name).into(),
                path: PathBuf::new(),
                index: 0,
            })
            .collect(),
    }
}

/// Where the System family's faces come from.
#[derive(Clone, Debug, Default)]
pub enum SystemFont {
    /// None: a watermark in the System font is refused.
    #[default]
    None,
    /// One file for every weight: a variable font is drawn at each, a static
    /// one at its own.
    File { path: PathBuf, index: u32 },
    /// The file and collection index of the face of a weight, looked up when
    /// that face is first drawn, as the desktop asks the platform.
    Lookup(fn(f32) -> Result<(PathBuf, u32)>),
}

/// The fonts text watermarks draw with: the system font and the folders the
/// installed families are listed from, both as given. The families are
/// listed once, on first use, and each system face read once; a clone
/// shares both.
#[derive(Clone, Default)]
pub struct Fonts(Arc<Shared>);

#[derive(Default)]
struct Shared {
    system: SystemFont,
    dirs: Vec<PathBuf>,
    families: OnceLock<Vec<Family>>,
    faces: [Mutex<Option<Font>>; 4],
}

impl std::fmt::Debug for Fonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fonts")
            .field("system", &self.0.system)
            .field("dirs", &self.0.dirs)
            .finish_non_exhaustive()
    }
}

impl Fonts {
    /// The System family from `system`, the installed ones from the font
    /// files in `dirs` (and their folders, four deep), in that order.
    pub fn new(system: SystemFont, dirs: Vec<PathBuf>) -> Self {
        Self(Arc::new(Shared {
            system,
            dirs,
            ..Default::default()
        }))
    }

    /// The families once listed, for an interface that must not wait for the
    /// listing; `None` until then.
    pub fn families_if_listed(&self) -> Option<&[Family]> {
        self.0.families.get().map(Vec::as_slice)
    }

    /// Every family: System first, then the installed ones by name. Reading
    /// their names only touches each file's header and name table.
    pub fn families(&self) -> &[Family] {
        self.0.families.get_or_init(|| {
            // Warm system faces on the font-listing worker before the editor
            // uses them.
            for (style, _) in SYSTEM_FACES {
                let _ = self.load(SYSTEM, style);
            }
            let mut families = vec![system()];
            families.extend(installed(&self.0.dirs));
            families
        })
    }
}

fn installed(dirs: &[PathBuf]) -> Vec<Family> {
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
    for dir in dirs {
        walk(dir, 0, &mut files);
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
            if family.starts_with('.') {
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

impl Fonts {
    /// Loads an installed face or a face of the system font.
    pub fn load(&self, family: &str, face: &str) -> Result<Font> {
        #[cfg(test)]
        if family == "Test fixture" {
            let weight = SYSTEM_FACES
                .iter()
                .find(|(name, _)| *name == face)
                .map(|(_, weight)| *weight)
                .context("Unknown fixture font style")?;
            return Ok(Font {
                data: Arc::new(include_bytes!("../../tests/fonts/InterVariable.ttf").to_vec()),
                index: 0,
                weight: Some(weight),
            });
        }
        if family == SYSTEM {
            let slot = SYSTEM_FACES
                .iter()
                .position(|(name, _)| *name == face)
                .context("Unknown system font style")?;
            let mut cached = self.0.faces[slot]
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(font) = cached.as_ref() {
                return Ok(font.clone());
            }
            let weight = SYSTEM_FACES[slot].1;
            let (path, index) = match &self.0.system {
                SystemFont::None => bail!("There is no system font for watermarks"),
                SystemFont::File { path, index } => (path.clone(), *index),
                SystemFont::Lookup(lookup) => lookup(weight)?,
            };
            let font = Font {
                data: Arc::new(std::fs::read(path).context("System watermark font not readable")?),
                index,
                weight: Some(weight),
            };
            let face = font.face()?;
            let glyph = face
                .charmap()
                .map('A')
                .context("System font has no Latin text")?;
            anyhow::ensure!(
                face.outline_glyphs().get(glyph).is_some(),
                "System font has no outlines"
            );
            *cached = Some(font.clone());
            return Ok(font);
        }
        let Some(found) = self
            .families()
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
}

/// Keeps rendering fixtures independent of installed fonts and macOS versions.
#[cfg(test)]
pub(crate) fn test_watermark() -> super::Watermark {
    super::Watermark {
        family: "Test fixture".into(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_font_given_is_drawn_at_every_weight() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fonts/InterVariable.ttf");
        let fonts = Fonts::new(
            SystemFont::File {
                path: fixture.into(),
                index: 0,
            },
            Vec::new(),
        );
        for (style, weight) in SYSTEM_FACES {
            let font = fonts.load(SYSTEM, style).unwrap();
            assert_eq!(font.weight, Some(weight));
            assert!(super::super::raster::measure(&font, "Ag 0123456789", 32.).is_some());
        }
        assert_eq!(fonts.families(), [system()]);
    }

    #[test]
    fn without_a_system_font_a_system_watermark_is_refused() {
        let error = Fonts::default().load(SYSTEM, "Regular").err().unwrap();
        assert_eq!(error.to_string(), "There is no system font for watermarks");
    }

    #[test]
    fn installed_families_are_listed_from_the_folders_given() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::copy(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fonts/InterVariable.ttf"),
            nested.join("Inter.ttf"),
        )
        .unwrap();
        let fonts = Fonts::new(SystemFont::None, vec![dir.path().into()]);
        assert!(fonts.families_if_listed().is_none());
        let names: Vec<_> = fonts.families().iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, [SYSTEM, "Inter Variable"]);
        assert!(fonts.families_if_listed().is_some());
        let face = &fonts.families()[1].faces[0].name;
        assert!(fonts.load("Inter Variable", face).is_ok());
        assert!(Fonts::default().families_if_listed().is_none());
    }
}
