//! XMP look profiles backed by Adobe DNG SDK-format HSV big tables and RGB tables.
//! Assets are read from the user's installation, never bundled with RAWmakase.
use super::{CameraProfile, Table, look_settings::LookSettings, rgb_table::RgbTable};
use crate::{
    color::curve::{CurveLut, ToneCurve},
    color::{srgb_decode, srgb_encode},
    xml::ns::{CRS, RDF, XML},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path, sync::Arc};
/// A look with Lightroom's Profile Amount (`crs:SupportsAmount`). The slider runs
/// 0–200%; the look's table follows it only between the bounds its table stores
/// (version 2 tables; a version 1 table stays at 100%).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AmountRange {
    pub table_min: f32,
    pub table_max: f32,
}
impl AmountRange {
    /// The strength of the look's table at a Profile Amount.
    pub fn table(&self, amount: f32) -> f32 {
        amount.clamp(self.table_min, self.table_max)
    }
}
/// A decoded look table and the amount bounds it stores.
struct DecodedTable {
    table: Table,
    bounds: [f32; 2],
}
fn is_zero(v: &f32) -> bool {
    *v == 0.
}
/// A look's RGB table and the amount it applies at.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RgbLook {
    pub(super) table: Arc<RgbTable>,
    /// The look's own amount for its table (`crs:RGBTableAmount`, 1 when absent).
    pub(super) look_amount: f32,
    /// The amount the table applies at, for the Profile Amount the look was resolved at.
    pub amount: f32,
}
impl RgbLook {
    fn new(table: RgbTable, look_amount: f32) -> Self {
        Self {
            amount: table.amount(look_amount, 1.),
            table: Arc::new(table),
            look_amount,
        }
    }
    /// Linear display RGB through the table.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        self.table.apply(rgb, self.amount)
    }
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn for_test(table: RgbTable, amount: f32) -> Self {
        Self {
            table: Arc::new(table),
            look_amount: 1.,
            amount,
        }
    }
    pub fn table(&self) -> &RgbTable {
        &self.table
    }
    pub fn amount(&self) -> f32 {
        self.amount
    }
}
const ALPHABET: &[u8] =
    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?`'|()[]{}@%$#";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Enhanced {
    pub uuid: String,
    pub base_name: String,
    /// Of a fitted look: the look of the camera JPEG it was fitted to, by the maker's
    /// name for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_look: Option<String>,
    #[serde(default)]
    pub highlights: f32,
    #[serde(default)]
    pub shadows: f32,
    #[serde(default)]
    pub clarity: f32,
    /// Contrast and Blacks, which Lightroom's B&W looks set. Omitted at zero, so
    /// releases that predate them read the profile.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub contrast: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub blacks: f32,
    #[serde(default)]
    pub monochrome: bool,
    /// Whether the look has Lightroom's Profile Amount, and how far its table goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<AmountRange>,
    /// The HSV look table; camera-matching looks with an RGB table may have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) table: Option<Table>,
    /// The RGB table of creative and camera-matching looks, which the colour stage
    /// applies after the colour mixer (see `develop::pipeline`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) rgb: Option<RgbLook>,
    pub(super) curve: Box<CurveLut>,
    /// Exposure, Saturation, colour mixer, parametric curve, split toning and
    /// vignette settings the look carries.
    #[serde(default, skip_serializing_if = "LookSettings::is_default")]
    pub settings: LookSettings,
}
impl Enhanced {
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn for_test(table: Table) -> Self {
        Self {
            camera_look: None,
            uuid: "0".repeat(32),
            base_name: "Test".into(),
            highlights: 0.,
            shadows: 0.,
            clarity: 0.,
            contrast: 0.,
            blacks: 0.,
            monochrome: false,
            amount: None,
            table: Some(table),
            rgb: None,
            settings: LookSettings::default(),
            curve: Box::new(CurveLut::from_fn(|i| {
                let x = i as f32 / 4096.;
                x * x * (3. - 2. * x)
            })),
        }
    }
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(!self.base_name.is_empty(), "Missing look base profile");
        self.validate_contents()
    }
    /// Everything but the base profile, which a parsed look file does not have yet.
    fn validate_contents(&self) -> Result<()> {
        self.settings.validate()?;
        ensure!(
            self.uuid.len() == 32 && self.uuid.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid look UUID"
        );
        ensure!(
            [
                self.highlights,
                self.shadows,
                self.clarity,
                self.contrast,
                self.blacks
            ]
            .iter()
            .all(|v| v.is_finite() && (-1. ..=1.).contains(v)),
            "Invalid profile tone adjustment"
        );
        if let Some(table) = &self.table {
            table.validate()?;
        }
        ensure!(
            self.table.is_some() || self.rgb.is_some(),
            "Look has no table"
        );
        ensure!(
            self.rgb.as_ref().is_none_or(|t| t.look_amount.is_finite()
                && (0. ..=2.).contains(&t.look_amount)
                && t.amount.is_finite()
                && (0. ..=4.).contains(&t.amount)),
            "Invalid RGB table amount"
        );
        ensure!(
            self.amount.is_none_or(
                |a| (0. ..=1.).contains(&a.table_min) && (1. ..=2.).contains(&a.table_max)
            ),
            "Invalid look amount bounds"
        );
        ensure!(
            self.curve
                .values()
                .iter()
                .all(|v| v.is_finite() && (0. ..=1.).contains(v)),
            "Invalid look curve"
        );
        Ok(())
    }
    /// The look at a Profile Amount (1 is 100%), as Camera Raw 18.7 renders it. Up to
    /// 100% the table's shifts and scales, the curve's change and the internal
    /// adjustments grow from none. Above, the table's shifts keep growing (within the
    /// bounds its table stores), the curve is applied again at the excess, and the
    /// adjustments grow at half the rate (+40 Shadows is +60 at 200%). An RGB table
    /// applies at the Profile Amount times the look's own amount, within its bounds.
    /// A look without an Amount stays at 100%.
    pub fn at_amount(&self, amount: f32) -> Self {
        let Some(range) = self.amount.filter(|_| amount != 1.) else {
            return self.clone();
        };
        let strength = range.table(amount);
        let table = self.table.as_ref().map(|t| t.scaled(strength));
        let rgb = self.rgb.as_ref().map(|t| RgbLook {
            amount: t.table.amount(t.look_amount, amount),
            ..t.clone()
        });
        let adjustment = if amount > 1. {
            1. + (amount - 1.) * 0.5
        } else {
            amount
        };
        let curve = &self.curve;
        Self {
            highlights: self.highlights * adjustment,
            shadows: self.shadows * adjustment,
            clarity: self.clarity * adjustment,
            contrast: self.contrast * adjustment,
            blacks: self.blacks * adjustment,
            settings: self.settings.scaled(adjustment),
            table,
            rgb,
            curve: Box::new(CurveLut::from_fn(|i| {
                let y = curve.values()[i];
                if amount > 1. {
                    y + (curve.evaluate_as_look(y) - y) * (amount - 1.)
                } else {
                    let x = i as f32 / 4096.;
                    x + (y - x) * amount
                }
            })),
            ..self.clone()
        }
    }
    pub(super) fn apply_table(&self, rgb: [f32; 3]) -> [f32; 3] {
        match &self.table {
            Some(table) => table.apply(rgb.map(|v| v.clamp(0., 1.)), None, 0.),
            None => rgb,
        }
    }
    pub fn rgb(&self) -> Option<&RgbLook> {
        self.rgb.as_ref()
    }
    /// Whether the look has an RGB table, lacks an HSV one or carries settings, which
    /// releases before RGB tables can't read.
    pub fn has_rgb_table_or_settings(&self) -> bool {
        self.rgb.is_some() || self.table.is_none() || !self.settings.is_default()
    }
    pub(super) fn apply_curve(&self, rgb: [f32; 3]) -> [f32; 3] {
        let p = rgb.map(|v| srgb_encode(v.clamp(0., 1.)));
        let eval = |v: f32| self.curve.evaluate_as_look(v);
        let lo = p.into_iter().fold(f32::INFINITY, f32::min);
        let hi = p.into_iter().fold(0., f32::max);
        let a = eval(lo);
        let b = eval(hi);
        if hi - lo > 1e-8 {
            p.map(|v| srgb_decode(a + (b - a) * (v - lo) / (hi - lo)))
        } else {
            [srgb_decode(a); 3]
        }
    }
}
/// A table attribute's bytes: Adobe's base 85 over zlib, with the expanded length
/// first. HSV look tables and RGB tables share it.
pub(super) fn expand(text: &str) -> Result<Vec<u8>> {
    ensure!(text.len() <= 16_000_000, "Look table too large");
    let digits: Vec<_> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    ensure!(digits.len() % 5 != 1, "Truncated base85 table");
    let mut compressed = Vec::with_capacity(digits.len() * 4 / 5);
    for chunk in digits.chunks(5) {
        let mut value = 0u64;
        let mut power = 1u64;
        for b in chunk {
            value += ALPHABET
                .iter()
                .position(|c| c == b)
                .context("Invalid base85 table")? as u64
                * power;
            power *= 85;
        }
        ensure!(value <= u32::MAX as u64, "Base85 overflow");
        compressed.extend_from_slice(&(value as u32).to_le_bytes()[..chunk.len() - 1]);
    }
    ensure!(compressed.len() >= 5, "Truncated compressed table");
    let expected = u32::from_le_bytes(compressed[..4].try_into()?) as usize;
    ensure!(expected <= 12_000_048, "Expanded look table too large");
    let mut data = Vec::new();
    flate2::read::ZlibDecoder::new(&compressed[4..])
        .take(expected as u64 + 1)
        .read_to_end(&mut data)?;
    ensure!(data.len() == expected, "Invalid expanded table size");
    Ok(data)
}
/// A table's bytes in Adobe's encoding, the inverse of [`expand`]: zlib with the
/// expanded length first, in base 85.
pub(super) fn compress(bytes: &[u8]) -> String {
    use std::io::Write;
    let mut compressed = (bytes.len() as u32).to_le_bytes().to_vec();
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    // Writing into memory cannot fail.
    z.write_all(bytes).expect("zlib into memory");
    compressed.extend(z.finish().expect("zlib into memory"));
    let mut text = String::with_capacity(compressed.len() * 5 / 4 + 5);
    for chunk in compressed.chunks(4) {
        let mut word = [0; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        let mut value = u32::from_le_bytes(word);
        for _ in 0..chunk.len() + 1 {
            text.push(ALPHABET[(value % 85) as usize] as char);
            value /= 85;
        }
    }
    text
}
fn decode_table(text: &str) -> Result<DecodedTable> {
    let data = expand(text)?;
    ensure!(data.len() >= 24, "Invalid expanded table size");
    let word = |i| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
    ensure!(
        word(0) == 0 && matches!(word(4), 1 | 2),
        "Unsupported big table type/version (requires HSV look table)"
    );
    let dims = [word(8) as usize, word(12) as usize, word(16) as usize];
    ensure!(
        dims.iter().all(|v| (1..=256).contains(v)),
        "Invalid table dimensions"
    );
    let count = dims.iter().product::<usize>();
    ensure!(count <= 1_000_000, "Look table too large");
    let end = 20 + count * 12;
    let tail = if word(4) == 2 { 20 } else { 4 };
    ensure!(
        data.len() == end + tail || data.len() == end + tail + 4,
        "Invalid table payload size"
    );
    ensure!(word(end) <= 1, "Unsupported table encoding");
    let mut bounds = [1.; 2];
    if word(4) == 2 {
        let min = f64::from_le_bytes(data[end + 4..end + 12].try_into()?);
        let max = f64::from_le_bytes(data[end + 12..end + 20].try_into()?);
        ensure!(
            min.is_finite() && max.is_finite() && (0. ..=1.).contains(&min) && max >= 1.,
            "Invalid table amount bounds"
        );
        bounds = [min as f32, max.min(2.) as f32];
    }
    if data.len() == end + tail + 4 {
        ensure!(word(end + tail) == 0, "Unsupported look table flags");
    }
    let table = Table {
        dims,
        srgb: word(end) == 1,
        data: data[20..end]
            .as_chunks::<12>()
            .0
            .iter()
            .map(|p| {
                std::array::from_fn(|c| f32::from_le_bytes(p[c * 4..c * 4 + 4].try_into().unwrap()))
            })
            .collect(),
    };
    table.validate()?;
    Ok(DecodedTable { table, bounds })
}
/// An XMP boolean, `None` when absent.
fn boolean(text: &str) -> Result<Option<bool>> {
    Ok(match text.to_ascii_lowercase().as_str() {
        "" => None,
        "true" => Some(true),
        "false" => Some(false),
        _ => anyhow::bail!("Invalid profile flag {text}"),
    })
}
/// Which base profile a look builds on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LookBase {
    /// A camera look (Adobe Color and the other Adobe Raw looks) over the named base,
    /// usually Adobe Standard.
    Named(String),
    /// A creative look (Lightroom's Artistic, B&W, Modern, Vintage) names no base: it
    /// goes over the photo's own profile.
    Any,
}
/// A look profile file, parsed and validated but not yet put over a base profile.
#[derive(Clone, Debug)]
pub(super) struct LookFile {
    pub name: String,
    /// Its `crs:Group`, which the profile menu lists it under; not part of the
    /// profile a recipe embeds.
    pub group: String,
    pub base: LookBase,
    restriction: String,
    copyright: String,
    look: Enhanced,
}
impl LookFile {
    pub(super) fn read(path: &Path) -> Result<Self> {
        ensure!(
            std::fs::metadata(path)?.len() <= 16_000_000,
            "XMP profile too large"
        );
        Self::parse(&std::fs::read_to_string(path)?)
    }
    /// Whether this look can go over `base`.
    pub(super) fn fits(&self, base: &CameraProfile) -> bool {
        base.enhanced.is_none()
            && match &self.base {
                LookBase::Named(name) => *name == base.name,
                LookBase::Any => true,
            }
    }
    /// The look over `base`, which carries the camera's matrices and tables.
    pub(super) fn compose(&self, base: &CameraProfile) -> Result<CameraProfile> {
        ensure!(
            self.fits(base),
            "Missing base camera profile {}",
            match &self.base {
                LookBase::Named(name) => name.as_str(),
                LookBase::Any => "",
            }
        );
        ensure!(
            self.restriction.is_empty() || self.restriction.eq_ignore_ascii_case(&base.camera),
            "Look belongs to {}",
            self.restriction
        );
        let mut p = base.clone();
        p.name = self.name.clone();
        p.copyright = format!("{}; {}", base.copyright, self.copyright);
        p.enhanced = Some(Enhanced {
            base_name: base.name.clone(),
            ..self.look.clone()
        });
        p.validate()?;
        Ok(p)
    }
    /// A look file's name and UUID, without reading its tables: what an import
    /// checks the names it gives against.
    pub(super) fn identity(text: &str) -> Result<(String, String)> {
        let doc = roxmltree::Document::parse(text)?;
        let d = look_description(&doc)?;
        Ok((
            look_name(d)?.into(),
            d.attribute((CRS, "UUID")).unwrap_or("").into(),
        ))
    }
    pub(super) fn parse(text: &str) -> Result<Self> {
        let doc = roxmltree::Document::parse(text)?;
        let d = look_description(&doc)?;
        let attr = |name| d.attribute((CRS, name)).unwrap_or("");
        let base = match attr("CameraProfile") {
            "" => LookBase::Any,
            name => LookBase::Named(name.into()),
        };
        // Fail closed: do not silently discard profile-internal develop controls.
        const META: &[&str] = &[
            "PresetType",
            "Cluster",
            "UUID",
            "SupportsAmount",
            "SupportsColor",
            "SupportsMonochrome",
            "SupportsHighDynamicRange",
            "SupportsNormalDynamicRange",
            "SupportsSceneReferred",
            "SupportsOutputReferred",
            "CameraModelRestriction",
            "Copyright",
            "ContactInfo",
            "Version",
            "ProcessVersion",
            "ConvertToGrayscale",
            "CameraProfile",
            "LookTable",
            "RGBTable",
            "RGBTableAmount",
            "RequiresRGBTables",
            "ShowInPresets",
            "ShowInQuickActions",
            "HasSettings",
            "Highlights2012",
            "Shadows2012",
            "Clarity2012",
            "Contrast2012",
            "Blacks2012",
        ];
        for a in d.attributes().filter(|a| a.namespace() == Some(CRS)) {
            ensure!(
                META.contains(&a.name())
                    || a.name().starts_with("Table_")
                    || LookSettings::reads(a.name()),
                "Unsupported profile setting {}",
                a.name()
            );
        }
        let monochrome = match attr("ConvertToGrayscale").to_ascii_lowercase().as_str() {
            "" | "false" => false,
            "true" => true,
            _ => anyhow::bail!("Invalid profile monochrome flag"),
        };
        let adjustment = |key| -> Result<f32> {
            let value = attr(key);
            Ok(if value.is_empty() {
                0.
            } else {
                value.parse::<f32>()? / 100.
            })
        };
        let name = look_name(d)?;
        ensure!(
            boolean(attr("RequiresRGBTables"))? != Some(true),
            "Look requires the photo's own RGB tables"
        );
        // A table attribute names its `Table_<md5>` attribute, which must be in the file.
        let table_text = |key: &'static str| -> Result<Option<&str>> {
            let id = attr(key);
            if id.is_empty() {
                return Ok(None);
            }
            ensure!(
                id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid {key}"
            );
            let text = d
                .attribute((CRS, format!("Table_{id}").as_str()))
                .with_context(|| format!("The look's {key} is not in the file"))?;
            Ok(Some(text))
        };
        let (table, bounds) = match table_text("LookTable")? {
            Some(text) => {
                let DecodedTable { table, bounds } = decode_table(text)?;
                (Some(table), bounds)
            }
            None => (None, [1.; 2]),
        };
        let rgb = match table_text("RGBTable")? {
            Some(text) => {
                let look_amount = match attr("RGBTableAmount") {
                    "" => 1.,
                    v => v.parse::<f32>().context("Invalid RGB table amount")?,
                };
                ensure!(
                    look_amount.is_finite() && (0. ..=2.).contains(&look_amount),
                    "Invalid RGB table amount"
                );
                Some(RgbLook::new(RgbTable::decode(text)?, look_amount))
            }
            None => None,
        };
        ensure!(table.is_some() || rgb.is_some(), "Missing look table");
        let amount = match attr("SupportsAmount").to_ascii_lowercase().as_str() {
            "" | "false" => None,
            "true" => Some(AmountRange {
                table_min: bounds[0],
                table_max: bounds[1],
            }),
            _ => anyhow::bail!("Invalid profile amount flag"),
        };
        let mut curve = ToneCurve::default();
        for n in d
            .children()
            .filter(|n| n.is_element() && n.tag_name().namespace() == Some(CRS))
        {
            let key = n.tag_name().name();
            if key.starts_with("ToneCurvePV2012") {
                let points = n
                    .descendants()
                    .filter(|v| v.has_tag_name((RDF, "li")))
                    .map(|v| -> Result<[f32; 2]> {
                        let (x, y) = v
                            .text()
                            .context("Empty curve point")?
                            .split_once(',')
                            .context("Invalid curve point")?;
                        Ok([
                            x.trim().parse::<f32>()? / 255.,
                            y.trim().parse::<f32>()? / 255.,
                        ])
                    })
                    .collect::<Result<Vec<_>>>()?;
                let parsed = ToneCurve {
                    points,
                    ..Default::default()
                };
                parsed.validate()?;
                if key == "ToneCurvePV2012" {
                    curve = parsed;
                } else {
                    ensure!(
                        [
                            "ToneCurvePV2012Red",
                            "ToneCurvePV2012Green",
                            "ToneCurvePV2012Blue"
                        ]
                        .contains(&key)
                            && parsed.points == [[0., 0.], [1., 1.]],
                        "Unsupported profile channel curve {key}"
                    );
                }
            } else {
                ensure!(
                    ["Name", "ShortName", "SortName", "Group", "Description"].contains(&key),
                    "Unsupported profile element {key}"
                );
            }
        }
        let lut = CurveLut::new(&curve);
        let look = Enhanced {
            camera_look: None,
            uuid: attr("UUID").into(),
            base_name: String::new(),
            highlights: adjustment("Highlights2012")?,
            shadows: adjustment("Shadows2012")?,
            clarity: adjustment("Clarity2012")?,
            contrast: adjustment("Contrast2012")?,
            blacks: adjustment("Blacks2012")?,
            monochrome,
            amount,
            table,
            rgb,
            curve: Box::new(lut),
            settings: LookSettings::parse(|key: &str| d.attribute((CRS, key)).unwrap_or(""))?,
        };
        look.validate_contents()?;
        Ok(Self {
            name: name.into(),
            group: label(d, "Group").unwrap_or("").trim().into(),
            base,
            restriction: attr("CameraModelRestriction").into(),
            copyright: attr("Copyright").into(),
            look,
        })
    }
}
/// A look file's profile description, which must be a look.
fn look_description<'a, 'input>(
    doc: &'a roxmltree::Document<'input>,
) -> Result<roxmltree::Node<'a, 'input>> {
    let d = doc
        .descendants()
        .find(|n| {
            n.has_tag_name((RDF, "Description"))
                && n.parent().is_some_and(|p| p.has_tag_name((RDF, "RDF")))
        })
        .context("Missing profile description")?;
    ensure!(
        d.attribute((CRS, "PresetType")) == Some("Look"),
        "XMP is not a look profile"
    );
    Ok(d)
}
/// The name a look lists under: its `crs:Name`'s default language.
fn look_name<'a>(d: roxmltree::Node<'a, '_>) -> Result<&'a str> {
    label(d, "Name").context("Missing profile name")
}
/// A label element's (`crs:Name`, `crs:Group`) default language.
fn label<'a>(d: roxmltree::Node<'a, '_>, element: &str) -> Option<&'a str> {
    d.children()
        .find(|n| n.has_tag_name((CRS, element)))
        .and_then(|n| {
            n.descendants().find(|n| {
                n.has_tag_name((RDF, "li")) && n.attribute((XML, "lang")) == Some("x-default")
            })
        })
        .and_then(|n| n.text())
}
#[cfg(any(test, feature = "test-support"))]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) mod tests {
    use super::*;
    /// A table's bytes in Adobe's encoding.
    pub(crate) fn encode(bytes: &[u8]) -> String {
        super::compress(bytes)
    }
    fn compose_text(text: &str, base: &CameraProfile) -> Result<CameraProfile> {
        LookFile::parse(text)?.compose(base)
    }
    fn fixture() -> String {
        let mut bytes = Vec::new();
        for v in [0u32, 1, 1, 2, 2] {
            bytes.extend(v.to_le_bytes());
        }
        for _ in 0..4 {
            for v in [120f32, 1., 1.] {
                bytes.extend(v.to_le_bytes());
            }
        }
        bytes.extend(0u32.to_le_bytes());
        encode(&bytes)
    }
    fn profile_xml(table: &str) -> String {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="{RDF}"><r:Description xmlns:c="{CRS}" c:PresetType="Look" c:UUID="0123456789ABCDEF0123456789ABCDEF" c:CameraProfile="Test base" c:LookTable="0123456789ABCDEF0123456789ABCDEF" c:Table_0123456789ABCDEF0123456789ABCDEF="{table}"><c:Name><r:Alt><r:li xml:lang="x-default">Test look</r:li></r:Alt></c:Name></r:Description></r:RDF></x:xmpmeta>"#
        )
    }
    #[test]
    fn sdk_base85_zlib_table_has_expected_hue_rotation() -> Result<()> {
        let t = decode_table(&fixture())?.table;
        let green = t.apply([0.5, 0., 0.], None, 0.);
        assert!((green[0]).abs() < 1e-6 && (green[1] - 0.5).abs() < 1e-6 && green[2].abs() < 1e-6);
        for s in ["", "x", "!!!!!", "zzzzzzzzzz", "\"\"\"\"\""] {
            assert!(decode_table(s).is_err());
        }
        Ok(())
    }
    #[test]
    #[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
    fn enhanced_profile_roundtrip_keeps_camera_and_old_profiles_unchanged() -> Result<()> {
        let mut base = CameraProfile::camera_matrix_default(&crate::camera_data::Metadata {
            make: "Fujifilm".into(),
            model: "X100F".into(),
            cam_xyz: [
                [1.1434, -0.4948, -0.121],
                [-0.3746, 1.2042, 0.1903],
                [-0.0666, 0.1479, 0.5235],
            ],
            ..Default::default()
        })
        .unwrap();
        base.name = "Test base".into();
        let xml = profile_xml(&fixture());
        let composed = compose_text(&xml, &base)?;
        assert_eq!(composed.camera, base.camera);
        assert!(base.enhanced.is_none());
        assert_eq!(composed.name, "Test look");
        let copy: CameraProfile = serde_json::from_slice(&serde_json::to_vec(&composed)?)?;
        assert_eq!(copy, composed);
        copy.validate()?;
        let old: CameraProfile = serde_json::from_slice(&serde_json::to_vec(&base)?)?;
        assert!(old.enhanced.is_none());
        assert!(
            compose_text(
                &xml.replace("c:PresetType=", "c:UnknownOperator=\"10\" c:PresetType="),
                &base
            )
            .is_err()
        );
        assert!(compose_text(&xml.replace("Test base", "Another camera profile"), &base).is_err());
        let look = copy.enhanced.as_ref().unwrap();
        for p in [[0.; 3], [0.18, 0.1, 0.3], [1.; 3]] {
            let result = look.apply_curve(p);
            assert!(result.into_iter().zip(p).all(|(a, b)| (a - b).abs() < 1e-5));
        }
        Ok(())
    }
    #[test]
    fn profile_amount_scales_the_look_as_camera_raw_does() {
        let mut look = Enhanced::for_test(decode_table(&fixture()).unwrap().table);
        look.shadows = 0.4;
        look.contrast = -0.2;
        // Without SupportsAmount the look stays at 100%.
        assert_eq!(look.at_amount(0.3), look);
        look.amount = Some(AmountRange {
            table_min: 0.,
            table_max: 2.,
        });
        assert_eq!(look.at_amount(1.), look);
        let none = look.at_amount(0.);
        assert!(none.table.unwrap().data.iter().all(|d| *d == [0., 1., 1.]));
        assert!(
            none.curve
                .values()
                .iter()
                .enumerate()
                .all(|(i, y)| (y - i as f32 / 4096.).abs() < 1e-6)
        );
        assert_eq!((none.shadows, none.contrast), (0., 0.));
        let half = look.at_amount(0.5);
        assert_eq!(half.table.unwrap().data[0], [60., 1., 1.]);
        assert_eq!(half.shadows, 0.2);
        // Above 100% the internal adjustments grow at half the rate, the table's
        // shifts keep growing and the curve is applied again.
        let double = look.at_amount(2.);
        assert_eq!(double.table.as_ref().unwrap().data[0], [240., 1., 1.]);
        assert!((double.shadows - 0.6).abs() < 1e-6);
        let mid = 2048;
        let once = look.curve.values()[mid];
        let twice = look.curve.values()[(once * 4096.).round() as usize];
        assert!(
            (double.curve.values()[mid] - twice).abs() < 1e-3,
            "{}",
            double.curve.values()[mid]
        );
        // A version 1 table has no amount bounds: it stays at 100%.
        look.amount = Some(AmountRange {
            table_min: 1.,
            table_max: 1.,
        });
        assert_eq!(look.at_amount(0.).table, look.table);
    }
    #[test]
    fn creative_looks_name_no_base_and_go_over_any_camera_profile() -> Result<()> {
        let mut base = CameraProfile::camera_matrix_default(&crate::camera_data::Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        })
        .unwrap();
        base.name = "Any base".into();
        let xml = profile_xml(&fixture())
            .replace(r#" c:CameraProfile="Test base""#, "")
            .replace("c:PresetType=", r#"c:SupportsAmount="True" c:PresetType="#);
        let file = LookFile::parse(&xml)?;
        assert_eq!(file.base, LookBase::Any);
        let composed = file.compose(&base)?;
        let look = composed.enhanced.as_ref().unwrap();
        assert_eq!(look.base_name, "Any base");
        // The fixture is a version 1 table: Amount is offered, the table stays at 100%.
        assert_eq!(
            look.amount,
            Some(AmountRange {
                table_min: 1.,
                table_max: 1.
            })
        );
        assert!(composed.supports_amount());
        // A look never goes over another look.
        assert!(file.compose(&composed).is_err());
        // Over Adobe Standard when there is one, else the first profile that fits:
        // the file's own, listed before RAWmakase Standard.
        let named = |name: &str| {
            let mut p = base.clone();
            p.name = name.into();
            std::sync::Arc::new(p)
        };
        // Listed by name, as `installed` returns them.
        let own = named("Zeta embedded");
        let mut bases = vec![
            std::sync::Arc::new(composed.clone()),
            named("Camera Standard"),
            named(super::super::open::STANDARD),
            own.clone(),
        ];
        let base_of = |bases: &[std::sync::Arc<CameraProfile>], own: Option<&CameraProfile>| {
            super::super::library::look_base(&file, bases, own).map(|b| b.name.clone())
        };
        assert_eq!(
            base_of(&bases, Some(&own)).as_deref(),
            Some("Zeta embedded")
        );
        assert_eq!(
            base_of(&bases, None).as_deref(),
            Some(super::super::open::STANDARD)
        );
        bases.push(named("Adobe Standard"));
        assert_eq!(
            base_of(&bases, Some(&own)).as_deref(),
            Some("Adobe Standard")
        );
        Ok(())
    }
    #[test]
    fn rgb_table_looks_parse_compose_and_scale_with_amount() -> Result<()> {
        use super::super::rgb_table::tests::{fade, table_bytes};
        let mut base = CameraProfile::camera_matrix_default(&crate::camera_data::Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        })
        .unwrap();
        base.name = "Camera Standard".into();
        let rgb = encode(&table_bytes(5, fade, (1, 3, 0, [0., 1.])));
        let id = "FEDCBA9876543210FEDCBA9876543210";
        // A camera-matching look: an RGB table over a named base, no HSV table.
        let xml = format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="{RDF}"><r:Description xmlns:c="{CRS}" c:PresetType="Look" c:UUID="0123456789ABCDEF0123456789ABCDEF" c:SupportsAmount="True" c:CameraProfile="Camera Standard" c:RequiresRGBTables="False" c:ShowInPresets="True" c:RGBTable="{id}" c:RGBTableAmount="0.5" c:Table_{id}="{rgb}"><c:Name><r:Alt><r:li xml:lang="x-default">Test match</r:li></r:Alt></c:Name></r:Description></r:RDF></x:xmpmeta>"#
        );
        let file = LookFile::parse(&xml)?;
        assert_eq!(file.base, LookBase::Named("Camera Standard".into()));
        let composed = file.compose(&base)?;
        let look = composed.enhanced.as_ref().unwrap();
        assert!(look.table.is_none() && look.has_rgb_table_or_settings());
        // The look's own amount at 100%; times the Profile Amount, within the
        // table's bounds (0–1) above.
        assert_eq!(look.rgb().unwrap().amount(), 0.5);
        assert_eq!(look.at_amount(0.5).rgb().unwrap().amount(), 0.25);
        assert_eq!(look.at_amount(2.).rgb().unwrap().amount(), 1.);
        // Saved in Adobe's encoding and read back.
        let copy: CameraProfile = serde_json::from_slice(&serde_json::to_vec(&composed)?)?;
        assert_eq!(copy, composed);
        copy.validate()?;
        // A table kept outside the file, or a look that needs Camera Raw's own tables,
        // is refused.
        let missing = xml.replace(
            &format!("c:Table_{id}="),
            "c:Table_00000000000000000000000000000000=",
        );
        let err = LookFile::parse(&missing).unwrap_err();
        assert!(format!("{err:#}").contains("not in the file"), "{err:#}");
        assert!(
            LookFile::parse(&xml.replace(
                r#"RequiresRGBTables="False""#,
                r#"RequiresRGBTables="True""#
            ))
            .is_err()
        );
        // Without either table there is nothing to render.
        assert!(LookFile::parse(&xml.replace(&format!(r#"c:RGBTable="{id}""#), "")).is_err());
        Ok(())
    }
    #[test]
    fn looks_carry_supported_settings_and_refuse_others() -> Result<()> {
        let xml = profile_xml(&fixture()).replace(
            "c:PresetType=",
            r#"c:Saturation="-20" c:SplitToningShadowHue="44" c:SplitToningShadowSaturation="25" c:PostCropVignetteAmount="-8" c:PresetType="#,
        );
        let file = LookFile::parse(&xml)?;
        assert_eq!(file.look.settings.saturation, -0.2);
        assert!(file.look.settings.toning.is_some() && file.look.settings.vignette.is_some());
        let copy: Enhanced = serde_json::from_slice(&serde_json::to_vec(&file.look)?)?;
        assert_eq!(copy, file.look);
        // Settings looks can't hold yet are still refused.
        let err = LookFile::parse(&xml.replace("c:Saturation=", "c:Dehaze=")).unwrap_err();
        assert!(format!("{err:#}").contains("Dehaze"), "{err:#}");
        Ok(())
    }
}
