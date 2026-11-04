//! Lensfun's lens database: its XML files read into cameras, mounts and lens
//! calibrations (`optics::lensfun`), the lens a photo matches ([`matching`]), the
//! choice an edit keeps ([`LensfunChoice`], with a copy of the calibration so the edit
//! renders without the database), and the installed copy ([`installed`]).
//!
//! The database is Lensfun's (<https://lensfun.github.io>), CC BY-SA 3.0, downloaded
//! on request and never bundled. Only its format is read here, from Lensfun's own
//! documentation: `<lensdatabase version="2">` with `<mount>` (`<name>`, `<compat>`),
//! `<camera>` (`<maker>`, `<model>`, `<variant>`, `<mount>`, `<cropfactor>`,
//! `<aspect-ratio>`) and `<lens>` (`<maker>`, `<model>`, `<mount>`, `<focal>`,
//! `<aperture>`, `<type>`, `<cropfactor>`, `<aspect-ratio>`, `<center>`,
//! `<calibration>`). A name without `lang` identifies; an `en` one, else it, is
//! shown.
pub mod choice;
pub mod installed;
pub mod matching;
pub mod names;

use crate::optics::lensfun::{
    Calibration, DEFAULT_ASPECT, Distortion, DistortionRow, LensType, MAX_ROWS, Tca, TcaRow,
    VignettingRow,
};
use anyhow::{Context, Result, bail, ensure};
pub use choice::{LensfunCamera, LensfunChoice, LensfunLens, Provenance};

/// The database format versions this reader understands.
pub const FORMATS: std::ops::RangeInclusive<u32> = 0..=2;
/// Largest database file read.
pub const MAX_FILE_BYTES: usize = 8 << 20;

/// A name as Lensfun gives it: the one that identifies (no `lang`), and the one to
/// show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Name {
    pub id: String,
    pub shown: String,
}
impl Name {
    fn read(node: roxmltree::Node, tag: &str) -> Option<Self> {
        let mut id = None;
        let mut en = None;
        for c in node.children().filter(|c| c.has_tag_name(tag)) {
            let text = c.text().unwrap_or("").trim();
            if text.is_empty() {
                continue;
            }
            match c.attribute("lang") {
                None => id = id.or(Some(text.to_string())),
                Some("en") => en = en.or(Some(text.to_string())),
                Some(_) => {}
            }
        }
        let id = id?;
        Some(Self {
            shown: en.unwrap_or_else(|| id.clone()),
            id,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mount {
    pub name: String,
    /// Mounts whose lenses fit a camera of this one (Lensfun's `<compat>`, one way).
    pub compat: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    pub maker: Name,
    pub model: Name,
    pub variant: String,
    pub mount: String,
    pub crop: f32,
    /// The frame's aspect ratio when Lensfun gives one.
    pub aspect: Option<f32>,
}
impl Camera {
    /// A fixed-lens camera: its mount is its own, named in lower case first.
    pub fn fixed_lens(&self) -> bool {
        self.mount.chars().next().is_some_and(char::is_lowercase)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lens {
    pub maker: Name,
    pub model: Name,
    pub mounts: Vec<String>,
    pub calibration: Calibration,
    /// Why it can't be used, though listed: a model RAWmakase doesn't evaluate.
    pub unsupported: Option<String>,
}
impl Lens {
    /// What an edit records to find it again.
    pub fn identity(&self) -> LensfunLens {
        LensfunLens {
            maker: self.maker.id.clone(),
            model: self.model.id.clone(),
            mounts: self.mounts.clone(),
            crop: self.calibration.crop,
            aspect: self.calibration.aspect,
        }
    }
    /// The maker and model, as shown.
    pub fn label(&self) -> String {
        self.model.shown.clone()
    }
    /// Why it can't correct a photo at all: its projection, or its models.
    pub fn refusal(&self) -> Option<String> {
        if self.calibration.lens_type != LensType::Rectilinear {
            return Some(format!(
                "a {} lens; its projection isn't supported",
                self.calibration.lens_type.name()
            ));
        }
        if let Some(why) = &self.unsupported {
            return Some(why.clone());
        }
        (!self.calibration.components().any()).then(|| "no calibration".to_string())
    }
}

/// The database, as read from its files.
#[derive(Clone, Debug, Default)]
pub struct Database {
    pub mounts: Vec<Mount>,
    pub cameras: Vec<Camera>,
    pub lenses: Vec<Lens>,
    /// The installed version's timestamp (seconds since 1970), 0 when not known.
    pub timestamp: i64,
}

impl Database {
    /// Reads every file's text; a file that doesn't parse fails the whole.
    pub fn from_texts<'a>(texts: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<Self> {
        let mut db = Self::default();
        for (name, text) in texts {
            db.add(text)
                .with_context(|| format!("Lensfun database file {name}"))?;
        }
        Ok(db)
    }
    /// Adds one file's mounts, cameras and lenses.
    pub fn add(&mut self, text: &str) -> Result<()> {
        ensure!(text.len() <= MAX_FILE_BYTES, "too large");
        // Lensfun's files name their DTD (external, never fetched); entities are
        // bounded by the node limit.
        let doc = roxmltree::Document::parse_with_options(
            text,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                nodes_limit: 1 << 20,
                ..Default::default()
            },
        )
        .context("not XML")?;
        let root = doc.root_element();
        ensure!(root.has_tag_name("lensdatabase"), "not a Lensfun database");
        let format: u32 = match root.attribute("version") {
            None => 0,
            Some(v) => v.trim().parse().context("bad version")?,
        };
        ensure!(
            FORMATS.contains(&format),
            "database format {format} isn't supported"
        );
        for node in root.children().filter(roxmltree::Node::is_element) {
            match node.tag_name().name() {
                "mount" => self.mounts.push(Mount {
                    name: text_of(node, "name").context("a mount without a name")?,
                    compat: node
                        .children()
                        .filter(|c| c.has_tag_name("compat"))
                        .filter_map(|c| c.text().map(|t| t.trim().to_string()))
                        .filter(|t| !t.is_empty())
                        .collect(),
                }),
                "camera" => self.cameras.push(camera(node)?),
                "lens" => self.lenses.push(lens(node)?),
                _ => {}
            }
        }
        ensure!(
            self.lenses
                .iter()
                .map(|l| l.calibration.distortion.len()
                    + l.calibration.tca.len()
                    + l.calibration.vignetting.len())
                .all(|n| n <= MAX_ROWS),
            "a lens with too many calibration rows"
        );
        Ok(())
    }
    /// The mount named `name`, ignoring case.
    pub fn mount(&self, name: &str) -> Option<&Mount> {
        self.mounts
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
    }
    /// The lens an edit's identity names: same maker, model, crop factor and aspect
    /// ratio, and a mount in common.
    pub fn find(&self, id: &LensfunLens) -> Option<&Lens> {
        self.lenses.iter().find(|l| {
            l.maker.id == id.maker
                && l.model.id == id.model
                && (l.calibration.crop - id.crop).abs() < 1e-4
                && (l.calibration.aspect - id.aspect).abs() < 1e-4
                && (id.mounts.is_empty() || l.mounts.iter().any(|m| id.mounts.contains(m)))
        })
    }
}

fn text_of(node: roxmltree::Node, tag: &str) -> Option<String> {
    node.children()
        .find(|c| c.has_tag_name(tag))
        .and_then(|c| c.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}
fn number(text: &str) -> Result<f32> {
    let v: f32 = text
        .trim()
        .parse()
        .with_context(|| format!("not a number: {text:?}"))?;
    ensure!(v.is_finite(), "not a finite number: {text:?}");
    Ok(v)
}
/// `<cropfactor>`.
fn crop_of(node: roxmltree::Node) -> Result<f32> {
    let crop = number(&text_of(node, "cropfactor").context("no crop factor")?)?;
    ensure!(crop > 0. && crop < 100., "bad crop factor {crop}");
    Ok(crop)
}
/// `<aspect-ratio>`: "3:2" or a number, long edge over short.
fn aspect_of(node: roxmltree::Node) -> Result<Option<f32>> {
    let Some(text) = text_of(node, "aspect-ratio") else {
        return Ok(None);
    };
    let ratio = match text.split_once(':') {
        Some((a, b)) => number(a)? / number(b)?,
        None => number(&text)?,
    };
    ensure!(ratio.is_finite() && ratio > 0., "bad aspect ratio {text}");
    Ok(Some(ratio.max(1. / ratio)))
}
fn camera(node: roxmltree::Node) -> Result<Camera> {
    let variant = text_of(node, "variant").unwrap_or_default();
    Ok(Camera {
        maker: Name::read(node, "maker").context("a camera without a maker")?,
        model: Name::read(node, "model").context("a camera without a model")?,
        mount: text_of(node, "mount").context("a camera without a mount")?,
        crop: crop_of(node)?,
        // Lensfun gives a body's aspect-ratio modes as variants ("16:9") with their
        // own crop factors.
        aspect: aspect_of(node)?.or_else(|| ratio(&variant)),
        variant,
    })
}
/// "16:9" as 16/9 (long edge over short); `None` for other variant names.
pub(crate) fn ratio(text: &str) -> Option<f32> {
    let (a, b) = text.trim().split_once(':')?;
    let (a, b): (f32, f32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (a > 0. && b > 0.).then(|| a.max(b) / a.min(b))
}
fn attr(node: roxmltree::Node, name: &str) -> Result<f32> {
    node.attribute(name).map_or(Ok(0.), number)
}
fn required(node: roxmltree::Node, name: &str) -> Result<f32> {
    number(node.attribute(name).with_context(|| format!("no {name}"))?)
}
fn lens(node: roxmltree::Node) -> Result<Lens> {
    let maker = Name::read(node, "maker").context("a lens without a maker")?;
    let model = Name::read(node, "model").context("a lens without a model")?;
    let mounts: Vec<String> = node
        .children()
        .filter(|c| c.has_tag_name("mount"))
        .filter_map(|c| c.text().map(|t| t.trim().to_string()))
        .filter(|t| !t.is_empty())
        .collect();
    ensure!(!mounts.is_empty(), "a lens without a mount");
    let mut c = Calibration {
        crop: crop_of(node)?,
        aspect: aspect_of(node)?.unwrap_or(DEFAULT_ASPECT),
        lens_type: LensType::parse(&text_of(node, "type").unwrap_or_default()),
        ..Default::default()
    };
    if let Some(centre) = node.children().find(|c| c.has_tag_name("center")) {
        c.centre = [attr(centre, "x")?, attr(centre, "y")?];
        ensure!(c.centre.iter().all(|v| v.abs() <= 1.), "bad centre");
    }
    let mut unsupported = Vec::new();
    for set in node.children().filter(|c| c.has_tag_name("calibration")) {
        for row in set.children().filter(roxmltree::Node::is_element) {
            let model = row.attribute("model").unwrap_or("");
            match row.tag_name().name() {
                "distortion" => {
                    let model = match model {
                        "none" => continue,
                        "poly3" => Distortion::Poly3([attr(row, "k1")?]),
                        "poly5" => Distortion::Poly5([attr(row, "k1")?, attr(row, "k2")?]),
                        "ptlens" => {
                            Distortion::Ptlens([attr(row, "a")?, attr(row, "b")?, attr(row, "c")?])
                        }
                        other => {
                            unsupported.push(format!("distortion model {other}"));
                            continue;
                        }
                    };
                    c.distortion.push(DistortionRow {
                        focal: positive(required(row, "focal")?)?,
                        model,
                    });
                }
                "tca" => {
                    let model = match model {
                        "none" => continue,
                        "linear" => Tca::Linear([attr_or(row, "kr", 1.)?, attr_or(row, "kb", 1.)?]),
                        "poly3" => Tca::Poly3([
                            attr_or(row, "vr", 1.)?,
                            attr_or(row, "vb", 1.)?,
                            attr(row, "cr")?,
                            attr(row, "cb")?,
                            attr(row, "br")?,
                            attr(row, "bb")?,
                        ]),
                        other => {
                            unsupported.push(format!("lateral CA model {other}"));
                            continue;
                        }
                    };
                    c.tca.push(TcaRow {
                        focal: positive(required(row, "focal")?)?,
                        model,
                    });
                }
                "vignetting" => {
                    match model {
                        "none" => continue,
                        "pa" => {}
                        other => {
                            unsupported.push(format!("vignetting model {other}"));
                            continue;
                        }
                    }
                    c.vignetting.push(VignettingRow {
                        focal: positive(required(row, "focal")?)?,
                        aperture: positive(required(row, "aperture")?)?,
                        distance: positive(required(row, "distance")?)?,
                        k: [attr(row, "k1")?, attr(row, "k2")?, attr(row, "k3")?],
                    });
                }
                _ => {}
            }
        }
    }
    // The focal range: `<focal>`, else what the name states, else the calibration's.
    let calibrated = c
        .distortion
        .iter()
        .map(|r| r.focal)
        .chain(c.tca.iter().map(|r| r.focal))
        .chain(c.vignetting.iter().map(|r| r.focal));
    let (lo, hi) = calibrated.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), f| {
        (lo.min(f), hi.max(f))
    });
    let declared = node.children().find(|c| c.has_tag_name("focal"));
    c.focal = match declared {
        Some(f) if f.attribute("value").is_some() => {
            let v = positive(required(f, "value")?)?;
            [v, v]
        }
        Some(f) => {
            let min = f.attribute("min").map(number).transpose()?.unwrap_or(lo);
            let max = f
                .attribute("max")
                .map(number)
                .transpose()?
                .unwrap_or(hi.max(min));
            [min, max]
        }
        // A range the name states in millimetres, when the calibration fits inside
        // it (a converter's, "+ 1.4x", lies outside the name's range).
        None => match names::stated_millimetres(&model.id) {
            Some([a, b])
                if a > 0. && b >= a && (!lo.is_finite() || (lo >= a * 0.97 && hi <= b * 1.03)) =>
            {
                [a, b]
            }
            _ => [lo, hi],
        },
    };
    if !(c.focal[0].is_finite() && c.focal[0] > 0. && c.focal[1] >= c.focal[0]) {
        // Neither declared nor calibrated: a lens listed for its name only.
        c.focal = [0., 0.];
    }
    let unsupported = (!unsupported.is_empty()).then(|| {
        unsupported.sort();
        unsupported.dedup();
        format!(
            "uses Lensfun's {}, which isn't supported",
            unsupported.join(", ")
        )
    });
    Ok(Lens {
        maker,
        model,
        mounts,
        calibration: c,
        unsupported,
    })
}
fn attr_or(node: roxmltree::Node, name: &str, default: f32) -> Result<f32> {
    node.attribute(name).map_or(Ok(default), number)
}
fn positive(v: f32) -> Result<f32> {
    if v > 0. {
        Ok(v)
    } else {
        bail!("not positive: {v}")
    }
}

/// A synthetic database and a photo it covers, made up for tests (never Lensfun's
/// data).
#[cfg(any(test, feature = "test-support"))]
pub mod fixture {
    use super::{Database, LensfunChoice, installed};
    use crate::camera_data::Metadata;
    use std::sync::Arc;

    /// A camera and two lenses: one with distortion, lateral CA and vignetting, and a
    /// second calibration `k1` stronger, as a later database version might have it.
    pub fn xml(k1: f32) -> String {
        format!(
            r#"<lensdatabase version="2">
<mount><name>Canon EF</name></mount>
<camera><maker>Canon</maker><model>Canon EOS 5D Mark III</model><mount>Canon EF</mount><cropfactor>1</cropfactor></camera>
<camera><maker>Canon</maker><model>Canon EOS 6D</model><mount>Canon EF</mount><cropfactor>1.005</cropfactor></camera>
<lens><maker>Canon</maker><model>Canon EF 70-200mm f/2.8L IS II USM</model><mount>Canon EF</mount><cropfactor>1</cropfactor>
<calibration>
<distortion model="poly3" focal="70" k1="{k1}"/><distortion model="poly3" focal="200" k1="0.01"/>
<tca model="linear" focal="70" kr="1.0004" kb="0.9997"/>
<vignetting model="pa" focal="70" aperture="4" distance="1000" k1="-0.5" k2="0.1" k3="0"/>
<vignetting model="pa" focal="200" aperture="4" distance="1000" k1="-0.3" k2="0" k3="0"/>
</calibration></lens>
</lensdatabase>"#
        )
    }
    pub fn database(k1: f32, timestamp: i64) -> Database {
        let mut db = Database::from_texts([("test.xml", xml(k1).as_str())]).unwrap();
        db.timestamp = timestamp;
        db
    }
    /// The database as if installed, for what resolves a Lensfun lens against it.
    pub fn installed(k1: f32, timestamp: i64) -> installed::Handle {
        let db = database(k1, timestamp);
        installed::Handle::Given(Some(Arc::new(installed::Installed {
            manifest: installed::Manifest {
                version: timestamp,
                url: super::choice::SOURCE_URL.into(),
                bytes: 0,
                sha256: String::new(),
                downloaded: String::new(),
                lenses: db.lenses.len(),
                cameras: db.cameras.len(),
            },
            db,
        })))
    }
    /// A photo the lens fits, at 100 mm and f/4, with no 35 mm equivalent focal
    /// length. Tests resolve it against [`none`], never the database installed on
    /// the computer.
    pub fn photo() -> Metadata {
        Metadata {
            make: "Canon".into(),
            model: "EOS 5D Mark III".into(),
            exif_make: "Canon".into(),
            exif_model: "Canon EOS 5D Mark III".into(),
            lens_model: "EF70-200mm f/2.8L IS II USM".into(),
            focal: 100.,
            aperture: 4.,
            width: 600,
            height: 400,
            ..Default::default()
        }
    }
    /// No database, as on a computer without Lensfun installed.
    pub fn none() -> installed::Handle {
        installed::Handle::Given(None)
    }
    /// The same lens on another body Lensfun knows.
    pub fn other_camera() -> Metadata {
        Metadata {
            model: "EOS 6D".into(),
            exif_model: "Canon EOS 6D".into(),
            ..photo()
        }
    }
    /// The lens chosen for `photo`, from the first database.
    pub fn choice() -> LensfunChoice {
        let db = database(-0.02, 1_793_180_205);
        let m = photo();
        LensfunChoice::of(&db.lenses[0], &db, super::choice::camera_for(&db, &m))
    }
}

#[cfg(test)]
mod tests;
