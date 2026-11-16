//! Fonts for text watermarks: the system default at its weights,
//! and the families installed on the computer with the styles each has.
use anyhow::{Context, Result, bail};
use skrifa::{FontRef, MetadataProvider, string::StringId};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// Resolves the default watermark from the fonts installed on this computer.
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

static FAMILIES: OnceLock<Vec<Family>> = OnceLock::new();

/// The families once listed, for an interface that must not wait for the
/// listing; `None` until then.
pub fn families_if_listed() -> Option<&'static [Family]> {
    FAMILIES.get().map(Vec::as_slice)
}

/// Every family: System first, then the installed ones by name. Installed
/// fonts are listed once, on first use; reading their names only touches
/// each file's header and name table.
pub fn families() -> &'static [Family] {
    FAMILIES.get_or_init(|| {
        // Warm system faces on the font-listing worker before the editor uses them.
        for (style, _) in SYSTEM_FACES {
            let _ = load(SYSTEM, style);
        }
        let mut families = vec![system()];
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

/// Loads an installed face or the platform's default watermark font.
pub fn load(family: &str, face: &str) -> Result<Font> {
    if family == SYSTEM {
        let slot = SYSTEM_FACES
            .iter()
            .position(|(name, _)| *name == face)
            .context("Unknown system font style")?;
        static FONTS: [OnceLock<Result<Font, String>>; 4] = [const { OnceLock::new() }; 4];
        return FONTS[slot]
            .get_or_init(|| {
                let weight = SYSTEM_FACES[slot].1;
                let load = || -> Result<Font> {
                    let (path, index) = system_face(weight)?;
                    let font = Font {
                        data: Arc::new(
                            std::fs::read(path).context("System watermark font not readable")?,
                        ),
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
                    Ok(font)
                };
                load().map_err(|error| format!("{error:#}"))
            })
            .clone()
            .map_err(anyhow::Error::msg);
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

fn system_face(weight: f32) -> Result<(PathBuf, u32)> {
    #[cfg(target_os = "macos")]
    {
        let _ = weight;
        Ok((PathBuf::from("/System/Library/Fonts/SFNS.ttf"), 0))
    }
    #[cfg(target_os = "windows")]
    {
        let root = std::env::var_os("SystemRoot").context("SystemRoot unavailable")?;
        let fonts = PathBuf::from(root).join("Fonts");
        Ok((
            if fonts.join("SegUIVar.ttf").is_file() {
                fonts.join("SegUIVar.ttf")
            } else {
                fonts.join(if weight >= 650. {
                    "segoeuib.ttf"
                } else if weight >= 500. {
                    "seguisb.ttf"
                } else {
                    "segoeui.ttf"
                })
            },
            0,
        ))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        use std::process::{Command, Stdio};
        let output = tempfile::tempfile()?;
        let mut child = Command::new("fc-match")
            .args([
                "-f",
                "%{file}\n%{index}",
                &format!(
                    "system-ui:weight={}",
                    if weight >= 650. {
                        200
                    } else if weight >= 550. {
                        180
                    } else if weight >= 450. {
                        100
                    } else {
                        80
                    }
                ),
            ])
            .stdout(output.try_clone()?)
            .stderr(Stdio::null())
            .spawn()
            .context("Cannot resolve system watermark font")?;
        let start = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                anyhow::ensure!(status.success(), "System font lookup failed");
                break;
            }
            if start.elapsed() > std::time::Duration::from_secs(1) {
                let _ = child.kill();
                let _ = child.wait();
                bail!("System font lookup timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        use std::io::{Read, Seek};
        let mut output = output;
        output.rewind()?;
        let mut text = String::new();
        output.read_to_string(&mut text)?;
        let mut lines = text.lines();
        let path = lines
            .next()
            .filter(|s| !s.is_empty())
            .context("No system font available")?;
        Ok((PathBuf::from(path), lines.next().unwrap_or("0").parse()?))
    }
}
