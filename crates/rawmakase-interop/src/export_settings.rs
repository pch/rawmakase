//! The choices of Lightroom's Export dialog: where the file goes, its name, format,
//! size and metadata. Saved as `export.json` in the data folder, so the next
//! export (and Export with Previous) starts from them.
use crate::color::OutputSpace;
use crate::storage::RasterFormat;
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

mod rename;
mod saved;
mod tokens;
pub use rename::RenameSettings;
pub use saved::{SavedTemplate, SavedTemplates};
pub use tokens::{
    BUILTIN, Builtin, DatePart, Def, DisplayError, Field, Menu, Needs, Part, Span, TOKENS, Token,
    TokenTemplate, builtin, by_label, menu,
};

/// The Export dialog's choice that takes its text from the photo's
/// copyright, as Lightroom's Simple Copyright Watermark does.
pub const SIMPLE_COPYRIGHT: &str = "<simple-copyright>";

/// A photo's own export choices, saved with its edit.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExportOptions {
    pub quality: u8,
    pub max_edge: u32,
    /// A JPEG XL export is lossy at `quality` rather than lossless: the Export
    /// dialog's choice ([`ExportSettings::jxl_lossy`]). Never saved with an edit, so
    /// the catalog's records are unchanged; lossless otherwise.
    #[serde(skip)]
    pub jxl_lossy: bool,
}
impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            quality: 92,
            max_edge: 0,
            jxl_lossy: false,
        }
    }
}
impl ExportOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=100).contains(&self.quality),
            "JPEG quality must be 1–100"
        );
        ensure!(
            self.max_edge <= 30_000,
            "Export edge must not exceed 30000 pixels"
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Destination {
    #[default]
    SameFolder,
    Desktop,
    Pictures,
    Folder,
}
impl Destination {
    pub const ALL: [Self; 4] = [
        Self::SameFolder,
        Self::Desktop,
        Self::Pictures,
        Self::Folder,
    ];
}
/// What to do when the exported file already exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Existing {
    #[default]
    Ask,
    Unique,
    Overwrite,
    Skip,
}
impl Existing {
    pub const ALL: [Self; 4] = [Self::Ask, Self::Unique, Self::Overwrite, Self::Skip];
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Format {
    #[default]
    Jpeg,
    Tiff,
    Png,
    /// JPEG XL, lossy or lossless ([`ExportSettings::jxl_lossy`]): 8-bit sRGB, or
    /// an HDR export's 16-bit PQ or HLG.
    Jxl,
}
impl Format {
    /// The formats an export can be written in.
    pub const ALL: [Self; 4] = [Self::Jpeg, Self::Tiff, Self::Png, Self::Jxl];

    pub fn raster(self) -> RasterFormat {
        match self {
            Format::Jpeg => RasterFormat::Jpeg,
            Format::Tiff => RasterFormat::Tiff,
            Format::Png => RasterFormat::Png,
            Format::Jxl => RasterFormat::Jxl,
        }
    }
    /// The extension an export is named with.
    pub fn extension(self) -> &'static str {
        self.raster().extension()
    }
    pub fn mime_type(self) -> &'static str {
        self.raster().mime_type()
    }
    /// The format `path`'s extension names, in any case: .jpg, .jpeg, .tif,
    /// .tiff or .png.
    pub fn from_path(path: &Path) -> Option<Format> {
        let raster = RasterFormat::of(path)?;
        Self::ALL.into_iter().find(|f| f.raster() == raster)
    }
    /// Whether an export in this format can be in `space`: ProPhoto RGB is written
    /// only as 16-bit TIFF, as 8 bits band in so wide a gamut.
    pub fn supports(self, space: OutputSpace) -> Result<()> {
        if self == Format::Jxl && space != OutputSpace::Srgb {
            bail!("JPEG XL exports are sRGB, or HDR; choose sRGB, HDR export or another format");
        }
        if space.needs_sixteen_bits() && self != Format::Tiff {
            bail!(
                "{} exports only as 16-bit TIFF; choose TIFF or another color space",
                space.label()
            );
        }
        Ok(())
    }
    /// Whether an HDR export can be written in this format: TIFF and JPEG XL hold
    /// HDR pixels, JPEG a gain map.
    pub fn holds_hdr(self) -> bool {
        matches!(self, Format::Tiff | Format::Jxl | Format::Jpeg)
    }
    /// Refuses an HDR export of `output` that `format` can't hold: a gain map in
    /// anything but JPEG (and in another gamut than Rec. 709, its sRGB base's), HDR
    /// pixels in anything but TIFF and JPEG XL, and linear light in anything but TIFF.
    pub fn check_hdr(format: Format, output: crate::color::hdr_output::HdrOutput) -> Result<()> {
        use crate::color::hdr_output::HdrGamut;
        if output.gain_map {
            ensure!(
                matches!(format, Format::Jpeg | Format::Jxl),
                "Gain maps are written as JPEG or JPEG XL; choose one, or HDR pixels in TIFF"
            );
            ensure!(
                output.gamut == HdrGamut::Rec709,
                "A gain map's base is sRGB: choose the Rec. 709 gamut"
            );
            return Ok(());
        }
        ensure!(
            matches!(format, Format::Tiff | Format::Jxl),
            "HDR pixels are written as TIFF or JPEG XL; choose one, or a gain map for JPEG"
        );
        ensure!(
            format == Format::Tiff
                || output.transfer != crate::color::hdr_output::HdrTransfer::Linear,
            "Linear HDR is written as 32-bit float TIFF only; choose TIFF, PQ or HLG"
        );
        Ok(())
    }
    /// Every extension an export can have, as a sentence lists them:
    /// ".jpg, .jpeg, .tif, .tiff or .png".
    pub fn extensions_listed() -> String {
        let extensions: Vec<String> = Self::ALL
            .into_iter()
            .flat_map(|f| f.raster().extensions())
            .map(|e| format!(".{e}"))
            .collect();
        match extensions.split_last() {
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
            None => String::new(),
        }
    }
}

/// Lightroom's Rename To templates that RAWmakase can fill, and the
/// "Filename - text" renaming of earlier releases.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Naming {
    #[default]
    Filename,
    /// "DSC0001-text": earlier releases' Rename To, kept for settings saved
    /// with it.
    FilenameText,
    FilenameSequence,
    CustomName,
    CustomNameSequence,
    /// "Text (1 of 3)".
    CustomNameOf,
    /// "20260504-DSC0001", from the capture date.
    DateFilename,
}
impl Naming {
    pub const ALL: [Self; 7] = [
        Self::Filename,
        Self::FilenameSequence,
        Self::FilenameText,
        Self::CustomName,
        Self::CustomNameSequence,
        Self::CustomNameOf,
        Self::DateFilename,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Filename => "Filename",
            Self::FilenameText => "Filename - Custom Text",
            Self::FilenameSequence => "Filename - Sequence",
            Self::CustomName => "Custom Name",
            Self::CustomNameSequence => "Custom Name - Sequence",
            Self::CustomNameOf => "Custom Name (x of y)",
            Self::DateFilename => "Date - Filename",
        }
    }
    pub fn uses_text(self) -> bool {
        matches!(
            self,
            Self::FilenameText | Self::CustomName | Self::CustomNameSequence | Self::CustomNameOf
        )
    }
    pub fn uses_sequence(self) -> bool {
        matches!(
            self,
            Self::FilenameSequence | Self::CustomNameSequence | Self::CustomNameOf
        )
    }
    /// The template as Lightroom's tokens, which the Filename Template
    /// Editor starts from.
    pub fn tokens(self) -> &'static str {
        match self {
            Self::Filename => "{{image_name}}",
            Self::FilenameText => "{{image_name}}-{{custom_token}}",
            Self::FilenameSequence => "{{image_name}}-{{naming_sequenceNumber_1Digit}}",
            Self::CustomName => "{{custom_token}}",
            Self::CustomNameSequence => "{{custom_token}}-{{naming_sequenceNumber_1Digit}}",
            Self::CustomNameOf => {
                "{{custom_token}} ({{naming_sequenceNumber_1Digit}} of {{naming_sequenceTotal_1Digit}})"
            }
            Self::DateFilename => "{{date_YYYYMMDD}}-{{image_name}}",
        }
    }
    /// The preset whose tokens `tokens` are, if any.
    pub fn from_tokens(tokens: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|n| n.tokens() == tokens)
    }
}

/// A camera's make and model for file names, once read from its file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CameraNames {
    /// Not read (yet, or the reading failed).
    #[default]
    NotRead,
    /// Read: the maker as RAWmakase spells it, and the model as the file
    /// writes it; `None` when the file has none.
    Read {
        make: Option<String>,
        model: Option<String>,
    },
}

/// What a photo's name can be made from, taken once for each export: the
/// Example and the files named all come from one reading. Text left empty
/// is unknown or unset.
///
/// Title, caption, copyright and creator are `None` while the catalog has
/// none of its own for the photo: the file's own then names it, once read
/// (`Some("")` is one cleared, or set empty, which the file never fills).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NameFields {
    /// Its capture time as the catalog keeps it ("2026-05-04 10:21:33.000")
    /// or EXIF writes it.
    pub captured: Option<String>,
    /// A virtual copy's name.
    pub copy_name: String,
    /// The name its file had when it entered the catalog (its master's for
    /// a virtual copy), with its extension; `None` when the catalog doesn't
    /// know it, and the Original filename token then gives the current name.
    pub original_name: Option<String>,
    /// Lens, exposure, focal length, ISO and dimensions; its camera is not
    /// used, see `camera`.
    pub info: crate::metadata::PhotoInfo,
    pub title: Option<String>,
    pub caption: Option<String>,
    pub copyright: Option<String>,
    /// Its creators, ", " between them, as Lightroom names them.
    pub creator: Option<String>,
    /// Its first creator (Lightroom's Artist), kept as `creator` is.
    pub artist: Option<String>,
    /// Its keywords' names, by name, ", " between them, as Lightroom names
    /// them.
    pub keywords: String,
    pub rating: i32,
    pub label: String,
    pub camera: CameraNames,
    /// The system's decimal separator, where Lightroom writes it.
    pub decimal: Decimal,
    /// What only its file says, once read.
    pub facts: FileFacts,
}

/// What file names take from a photo's file alone, the catalog keeping none
/// of it; each `None` when the file doesn't say (or wasn't read).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileFacts {
    /// The software that wrote it (EXIF Software).
    pub software: Option<String>,
    /// Its focal length in 35 mm film (EXIF FocalLengthIn35mmFilm).
    pub focal_35mm: Option<u32>,
    /// Its IPTC headline and place names (XMP).
    pub headline: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub sublocation: Option<String>,
}

/// The decimal separator of the user's region, which Lightroom writes in an
/// Exposure's aperture ("f - 5,0" in Poland); other numbers in names keep
/// `.`, as Lightroom's do. Taken once with a scope's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decimal(pub char);
impl Default for Decimal {
    fn default() -> Self {
        Decimal('.')
    }
}
impl NameFields {
    /// Nothing known.
    pub const NONE: NameFields = NameFields {
        captured: None,
        copy_name: String::new(),
        original_name: None,
        info: crate::metadata::PhotoInfo {
            camera: None,
            lens: None,
            focal: None,
            aperture: None,
            exposure: None,
            iso: None,
            dimensions: None,
        },
        title: None,
        caption: None,
        copyright: None,
        creator: None,
        artist: None,
        keywords: String::new(),
        rating: 0,
        label: String::new(),
        camera: CameraNames::NotRead,
        decimal: Decimal('.'),
        facts: FileFacts {
            software: None,
            focal_35mm: None,
            headline: None,
            city: None,
            state: None,
            country: None,
            sublocation: None,
        },
    };
    /// Whether a title, caption, copyright or creator is the file's own,
    /// which the catalog doesn't have.
    pub fn lacks_descriptive(&self) -> bool {
        [&self.title, &self.caption, &self.copyright, &self.creator]
            .iter()
            .any(|t| t.is_none())
    }
    /// Only its capture time known.
    pub fn captured(captured: Option<String>) -> Self {
        Self {
            captured,
            ..Self::default()
        }
    }
}
static NO_FIELDS: NameFields = NameFields::NONE;

/// Where a photo stands in its export, for the names that number or date it.
#[derive(Clone, Copy, Debug)]
pub struct NameContext<'a> {
    /// Its place in the export, from 0, in the order the photos were chosen.
    pub index: usize,
    pub total: usize,
    /// What its name can be made from.
    pub fields: &'a NameFields,
}
impl NameContext<'_> {
    /// One photo exported on its own, nothing known about it.
    pub const ONE: NameContext<'static> = NameContext {
        index: 0,
        total: 1,
        fields: &NO_FIELDS,
    };
}

/// A Lightroom naming template, filled: what names a file, before its
/// extension, wherever a file is named (an export, a rename).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameTemplate {
    pub naming: Naming,
    /// The custom text.
    pub text: String,
    /// Where Sequence starts.
    pub start: u32,
    /// How many digits Sequence has at least, zeros in front ("007").
    pub padding: usize,
    /// A template made in the Filename Template Editor, which names files in
    /// place of `naming`.
    pub tokens: Option<TokenTemplate>,
}
impl NameTemplate {
    /// `source`'s new name without its extension, for its place `at`, and
    /// what it lacks of what the template needs (a capture date…).
    pub fn stem(&self, source: &Path, at: &NameContext) -> (String, Vec<String>) {
        if let Some(tokens) = &self.tokens {
            return tokens.stem(source, at, &self.text, self.start);
        }
        let stem = source.file_stem().unwrap_or_default().to_string_lossy();
        let text = name_part(&self.text);
        let text = text.as_str();
        // A custom name left empty is the file's own.
        let custom = if text.is_empty() { &*stem } else { text };
        let sequence = format!(
            "{:0width$}",
            self.start as usize + at.index,
            width = self.padding
        );
        let mut note = Vec::new();
        let name = match self.naming {
            Naming::Filename => stem.to_string(),
            Naming::FilenameText if text.is_empty() => stem.to_string(),
            Naming::FilenameText => format!("{stem}-{text}"),
            Naming::FilenameSequence => format!("{stem}-{sequence}"),
            Naming::CustomName => custom.to_string(),
            Naming::CustomNameSequence => format!("{custom}-{sequence}"),
            Naming::CustomNameOf => format!("{custom} ({sequence} of {})", at.total),
            Naming::DateFilename => match at.fields.captured.as_deref().and_then(date) {
                Some(date) => format!("{date}-{stem}"),
                // Never a file's modified date, which would look like a real
                // capture date.
                None => {
                    note.push("no capture date: named undated".into());
                    format!("undated-{stem}")
                }
            },
        };
        // Windows' device names can't be file names, whatever follows a dot.
        let head = name.split('.').next().unwrap_or_default();
        let name = if reserved(head) {
            format!("{head}_{}", &name[head.len()..])
        } else {
            name
        };
        (name, note)
    }
    /// What it needs that a photo's file may have to give.
    pub fn needs(&self) -> Needs {
        match &self.tokens {
            Some(tokens) => tokens.needs(),
            None => Needs {
                captured: self.naming == Naming::DateFilename,
                ..Needs::default()
            },
        }
    }
    /// Whether it has the Custom Text.
    pub fn uses_text(&self) -> bool {
        self.tokens
            .as_ref()
            .map_or(self.naming.uses_text(), TokenTemplate::uses_text)
    }
    /// Whether it numbers photos from Start Number.
    pub fn uses_sequence(&self) -> bool {
        self.tokens
            .as_ref()
            .map_or(self.naming.uses_sequence(), TokenTemplate::uses_sequence)
    }
}

/// Lightroom's Post-Processing: what happens once an export is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AfterExport {
    #[default]
    Nothing,
    /// The exported files, in the system's file manager.
    Show,
}
impl AfterExport {
    pub const ALL: [Self; 2] = [Self::Nothing, Self::Show];
    pub fn label(self) -> &'static str {
        match self {
            Self::Nothing => "Do Nothing",
            Self::Show if cfg!(target_os = "macos") => "Show in Finder",
            Self::Show if cfg!(windows) => "Show in Explorer",
            Self::Show => "Show in File Manager",
        }
    }
}

/// Lightroom's Metadata "Include" choice, or Custom: the four switches
/// earlier releases had, which settings saved by them become.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Include {
    CopyrightOnly,
    CopyrightAndContact,
    AllExceptCameraAndCameraRaw,
    AllExceptCameraRaw,
    All,
    #[default]
    Custom,
}
impl Include {
    pub const ALL: [Self; 6] = [
        Self::CopyrightOnly,
        Self::CopyrightAndContact,
        Self::AllExceptCameraRaw,
        Self::AllExceptCameraAndCameraRaw,
        Self::All,
        Self::Custom,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::CopyrightOnly => "Copyright Only",
            Self::CopyrightAndContact => "Copyright & Contact Info Only",
            Self::AllExceptCameraAndCameraRaw => "All Except Camera & Camera Raw Info",
            Self::AllExceptCameraRaw => "All Except Camera Raw Info",
            Self::All => "All Metadata",
            Self::Custom => "Custom",
        }
    }
    /// Lightroom always leaves the location out of these.
    pub fn removes_location(self) -> bool {
        matches!(self, Self::CopyrightOnly | Self::CopyrightAndContact)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub destination: Destination,
    pub folder: Option<PathBuf>,
    pub subfolder: bool,
    pub subfolder_name: String,
    pub existing: Existing,
    /// Settings saved before the naming templates: "Filename - text" when on.
    pub rename: bool,
    pub custom_text: String,
    /// The Rename To template; `None` in settings saved before there was a
    /// choice, which follow `rename`, and for one only a later release knows
    /// (see [`LaterNaming`]).
    #[serde(deserialize_with = "known_or_default")]
    pub naming: Option<Naming>,
    #[serde(skip)]
    pub later_naming: LaterNaming,
    /// The template made in the Filename Template Editor (or one of
    /// RAWmakase's own), which names files while Rename To is still the
    /// preset it was made over; see [`CustomNaming`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_template: Option<CustomNaming>,
    /// Where Sequence starts.
    pub start_number: u32,
    pub uppercase: bool,
    /// JPEG when the saved format is one only a later release knows; see
    /// [`LaterFormat`].
    #[serde(deserialize_with = "known_or_default")]
    pub format: Format,
    #[serde(skip)]
    pub later_format: LaterFormat,
    /// The colour space of the pixels and the embedded profile; sRGB in settings
    /// saved before there was a choice, and standing in for one only a later
    /// release knows (see [`LaterColorSpace`]), which is refused rather than
    /// exported as sRGB.
    #[serde(deserialize_with = "known_or_default")]
    pub color_space: OutputSpace,
    #[serde(skip)]
    pub later_color_space: LaterColorSpace,
    /// An HDR export's encoding (docs/hdr.md#hdr-export): the edit's HDR master in
    /// place of its SDR rendition, written as a TIFF. `None`, the default, exports
    /// SDR in `color_space`. Omitted when `None`, so releases that predate it read the
    /// settings. One only a later release knows reads as `None` here and is kept
    /// (see [`LaterHdr`]), refused rather than exported as SDR.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "known_or_default"
    )]
    pub hdr: Option<crate::color::hdr_output::HdrOutput>,
    #[serde(skip)]
    pub later_hdr: LaterHdr,
    pub quality: u8,
    /// A JPEG XL export is lossy, at `quality`, rather than lossless. New settings
    /// start lossy; settings saved before JPEG XL could be lossy lack it and read as
    /// lossless, as they exported then (the field's own default, not the struct's).
    #[serde(default)]
    pub jxl_lossy: bool,
    pub resize: bool,
    pub long_edge: u32,
    pub ppi: u32,
    /// Camera, capture settings, lens and dates (EXIF).
    pub capture: bool,
    pub location: bool,
    /// The edit as Camera Raw settings (XMP).
    pub develop: bool,
    /// Rating, color label, keywords, and the title, caption, creator and
    /// copyright set in RAWmakase (XMP, and EXIF with `capture`).
    pub descriptive: bool,
    /// Which metadata goes in; the four switches above apply to Custom.
    pub include: Include,
    /// Lightroom's Remove Location Info, for the modes other than Custom.
    pub remove_location: bool,
    /// Lightroom's Watermarking: whether to, and which: a preset's name or
    /// the Simple Copyright Watermark.
    pub watermark: bool,
    pub watermark_name: String,
    /// Lightroom's After Export.
    pub after_export: AfterExport,
    /// Settings a later release added; see [`LaterFields`].
    #[serde(flatten)]
    pub later_fields: LaterFields,
}
impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            destination: Destination::SameFolder,
            folder: None,
            subfolder: false,
            subfolder_name: "Exported".into(),
            existing: Existing::Ask,
            rename: false,
            custom_text: "edited".into(),
            naming: None,
            later_naming: LaterNaming::default(),
            name_template: None,
            start_number: 1,
            uppercase: false,
            format: Format::Jpeg,
            later_format: LaterFormat::default(),
            color_space: OutputSpace::Srgb,
            later_color_space: LaterColorSpace::default(),
            hdr: None,
            later_hdr: LaterHdr::default(),
            quality: 92,
            jxl_lossy: true,
            resize: false,
            long_edge: 2048,
            ppi: 240,
            capture: true,
            location: true,
            develop: true,
            descriptive: true,
            include: Include::Custom,
            remove_location: false,
            watermark: false,
            watermark_name: SIMPLE_COPYRIGHT.into(),
            after_export: AfterExport::Nothing,
            later_fields: LaterFields::default(),
        }
    }
}

/// The saved format when only a later release knows it. JPEG stands in for
/// it here, and it is saved back until a format is chosen, so that release
/// still finds its choice after this one saves the settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaterFormat(Option<serde_json::Value>);

/// The saved colour space when only a later release knows it. Kept and saved
/// back until a space is chosen, as [`LaterFormat`] is, but never exported as
/// another: [`ExportSettings::output_space`] refuses it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaterColorSpace(Option<serde_json::Value>);

/// The saved HDR export when only a later release knows its encoding. Kept and
/// saved back until HDR export is chosen here, as [`LaterColorSpace`] is, and never
/// exported as SDR: [`ExportSettings::hdr_output`] refuses it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaterHdr(Option<serde_json::Value>);

/// The saved Rename To when only a later release knows it, as
/// [`LaterFormat`] is for the format.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaterNaming(Option<serde_json::Value>);

/// A custom template in `export.json`: `{"tokens": "{{image_name}}…",
/// "over": "Filename", …}`. Earlier releases don't know it, keep it among
/// their [`LaterFields`] and name files by `naming`, which is never a value
/// they can't read. It names files only while `naming` is still `over`, the
/// preset it was made over: an earlier release that chose another preset
/// has the files named by that. Kept as read, so fields a later release adds
/// inside it are saved back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CustomNaming(serde_json::Value);
impl CustomNaming {
    /// Its tokens, in Lightroom's form.
    pub fn tokens(&self) -> Option<&str> {
        self.0.get("tokens")?.as_str()
    }
    fn over(&self) -> Option<&serde_json::Value> {
        self.0.get("over")
    }
}

/// The saved settings only a later release knows, as they were read. They
/// mean nothing here and are saved back untouched, so that release still finds
/// them after this one saves the settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LaterFields(serde_json::Map<String, serde_json::Value>);

/// A choice a later release added reads as the default, so an older one
/// keeps the rest of the settings.
fn known_or_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Read<T> {
        Known(T),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Read::deserialize(d)? {
        Read::Known(value) => value,
        Read::Other(_) => T::default(),
    })
}

fn path() -> PathBuf {
    crate::storage::data_dir().join("export.json")
}
/// The desktop's own Desktop and Pictures folders: on Linux those named in
/// user-dirs.dirs (in the desktop's language, or moved by the user), on
/// Windows those the shell reports (OneDrive backup moves them), and on macOS
/// those in the home folder. Without one, the folder of that name at home.
fn desktop() -> PathBuf {
    user_folder(directories::UserDirs::desktop_dir, "Desktop")
}
fn pictures() -> PathBuf {
    user_folder(directories::UserDirs::picture_dir, "Pictures")
}
fn user_folder(folder: fn(&directories::UserDirs) -> Option<&Path>, name: &str) -> PathBuf {
    let dirs = directories::UserDirs::new();
    match &dirs {
        Some(dirs) => folder(dirs).map_or_else(|| dirs.home_dir().join(name), Path::to_path_buf),
        // UserDirs needs the profile folder from the shell on Windows; the
        // environment may still name it.
        None => std::env::home_dir().unwrap_or_default().join(name),
    }
}

impl ExportSettings {
    /// The last export's choices, if there was one.
    pub fn load() -> Option<Self> {
        Self::load_from(&path())
    }
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&path())
    }
    /// The format the user picks, which replaces one from a later release.
    pub fn choose_format(&mut self, format: Format) {
        self.format = format;
        self.later_format = LaterFormat::default();
    }
    /// The HDR export the user picks (`None` for SDR), which replaces one from a
    /// later release.
    pub fn choose_hdr(&mut self, hdr: Option<crate::color::hdr_output::HdrOutput>) {
        self.hdr = hdr;
        self.later_hdr = LaterHdr::default();
    }
    /// The colour space the user picks, which replaces one from a later release.
    pub fn choose_color_space(&mut self, space: OutputSpace) {
        self.color_space = space;
        self.later_color_space = LaterColorSpace::default();
    }
    /// The saved colour space only a later release knows, as saved.
    pub fn later_color_space(&self) -> Option<&serde_json::Value> {
        self.later_color_space.0.as_ref()
    }
    /// The colour space to export in, checked with the format before anything
    /// renders: refused when only a later release knows it, and for ProPhoto RGB in
    /// a format other than TIFF. Every export checks its settings with this.
    pub fn output_space(&self) -> Result<OutputSpace> {
        if let Some(later) = self.later_color_space() {
            bail!(
                "The color space {later} is from a later version of RAWmakase; choose another in the Export dialog"
            );
        }
        self.format.supports(self.color_space)?;
        Ok(self.color_space)
    }
    /// The Rename To preset the user picks, which replaces a custom template
    /// and a choice from a later release.
    pub fn choose_naming(&mut self, naming: Naming) {
        self.naming = Some(naming);
        self.later_naming = LaterNaming::default();
        self.name_template = None;
    }
    /// The template the user makes or picks, in Lightroom's form: a preset's
    /// own tokens choose that preset, any other names files over the preset
    /// chosen now. What a later release added to the template it replaces is
    /// kept.
    pub fn choose_tokens(&mut self, tokens: &str) {
        if let Some(naming) = Naming::from_tokens(tokens) {
            return self.choose_naming(naming);
        }
        let naming = self.naming();
        self.naming = Some(naming);
        self.later_naming = LaterNaming::default();
        let mut template = match self.name_template.take() {
            Some(CustomNaming(serde_json::Value::Object(map))) => map,
            _ => serde_json::Map::new(),
        };
        template.insert("tokens".into(), tokens.into());
        template.insert(
            "over".into(),
            serde_json::to_value(naming).unwrap_or_default(),
        );
        self.name_template = Some(CustomNaming(template.into()));
    }
    /// The custom template that names files, in Lightroom's form: `None`
    /// when Rename To's preset does.
    pub fn custom_tokens(&self) -> Option<&str> {
        let template = self.name_template.as_ref()?;
        let over = serde_json::to_value(self.naming()).ok()?;
        (self.later_naming.0.is_none() && template.over() == Some(&over))
            .then(|| template.tokens())
            .flatten()
    }
    /// The settings saved at `path`, read as [`ExportSettings::load`] reads them.
    #[cfg(any(test, feature = "test-support"))]
    pub fn load_file(path: &Path) -> Option<Self> {
        Self::load_from(path)
    }
    fn load_from(path: &Path) -> Option<Self> {
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        let later_format = saved
            .get("format")
            .filter(|format| Format::deserialize(*format).is_err())
            .cloned();
        let later_color_space = saved
            .get("color_space")
            .filter(|space| OutputSpace::deserialize(*space).is_err())
            .cloned();
        let later_naming = saved
            .get("naming")
            .filter(|naming| Option::<Naming>::deserialize(*naming).is_err())
            .cloned();
        let later_hdr = saved
            .get("hdr")
            .filter(|hdr| Option::<crate::color::hdr_output::HdrOutput>::deserialize(*hdr).is_err())
            .cloned();
        let mut settings = Self::deserialize(saved).ok()?;
        settings.later_hdr = LaterHdr(later_hdr);
        settings.later_format = LaterFormat(later_format);
        settings.later_color_space = LaterColorSpace(later_color_space);
        settings.later_naming = LaterNaming(later_naming);
        Some(settings)
    }
    fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let mut saved = serde_json::to_value(self)?;
        // Still the stand-ins they were read as: the later release's choices.
        if let LaterFormat(Some(later)) = &self.later_format
            && self.format == Format::default()
        {
            saved["format"] = later.clone();
        }
        if let LaterColorSpace(Some(later)) = &self.later_color_space
            && self.color_space == OutputSpace::default()
        {
            saved["color_space"] = later.clone();
        }
        if let LaterNaming(Some(later)) = &self.later_naming
            && self.naming.is_none()
        {
            saved["naming"] = later.clone();
        }
        if let LaterHdr(Some(later)) = &self.later_hdr
            && self.hdr.is_none()
        {
            saved["hdr"] = later.clone();
        }
        crate::storage::atomic_json(path, &saved)
    }
    /// The chosen folder, before any subfolder; `None` until a specific
    /// folder is picked.
    pub fn base_folder(&self, source: &Path) -> Option<PathBuf> {
        match self.destination {
            Destination::SameFolder => source.parent().map(Path::to_path_buf),
            Destination::Desktop => Some(desktop()),
            Destination::Pictures => Some(pictures()),
            Destination::Folder => self.folder.clone(),
        }
    }
    /// The Rename To template: the one chosen, or what earlier releases'
    /// checkbox meant.
    pub fn naming(&self) -> Naming {
        self.naming.unwrap_or(if self.rename {
            Naming::FilenameText
        } else {
            Naming::Filename
        })
    }
    #[cfg(test)]
    pub fn file_name(&self, source: &Path) -> String {
        self.file_name_for(source, &NameContext::ONE).0
    }
    /// The Rename To template as these settings fill it.
    pub fn name_template(&self) -> NameTemplate {
        NameTemplate {
            naming: self.naming(),
            text: self.custom_text.clone(),
            start: self.start_number,
            padding: 0,
            tokens: self.custom_tokens().map(TokenTemplate::parse),
        }
    }
    /// `source`'s exported name, as the template makes it for its place in the
    /// export, and what it lacks of what the template needs (a capture date…).
    pub fn file_name_for(&self, source: &Path, at: &NameContext) -> (String, Vec<String>) {
        let (name, note) = self.name_template().stem(source, at);
        let extension = self.format.extension();
        let name = if self.uppercase {
            format!("{name}.{}", extension.to_uppercase())
        } else {
            format!("{name}.{extension}")
        };
        (name, note)
    }
    /// Where `source` exports to.
    pub fn target(&self, source: &Path) -> Option<PathBuf> {
        self.target_for(source, &NameContext::ONE)
            .map(|(path, _)| path)
    }
    /// Where `source` exports to at its place in the export, and what its name
    /// has to say.
    pub fn target_for(&self, source: &Path, at: &NameContext) -> Option<(PathBuf, Vec<String>)> {
        let mut folder = self.base_folder(source)?;
        let name = self.subfolder_name.trim();
        if self.subfolder && !name.is_empty() {
            folder.push(name);
        }
        let (file, note) = self.file_name_for(source, at);
        Some((folder.join(file), note))
    }
    /// The HDR encoding these settings export, if any: refused in a format that
    /// can't hold it (an HDR export is a TIFF until HDR JPEG XL is written).
    pub fn hdr_output(&self) -> Result<Option<crate::color::hdr_output::HdrOutput>> {
        if let LaterHdr(Some(later)) = &self.later_hdr {
            bail!(
                "The HDR export {later} is from a later version of RAWmakase; choose another in the Export dialog"
            );
        }
        if let Some(output) = self.hdr {
            Format::check_hdr(self.format, output)?;
            return Ok(Some(output));
        }
        Ok(None)
    }
    /// What every export checks before anything renders: an HDR export's encoding in
    /// its format, else the SDR colour space in its format ([`Self::output_space`]).
    /// An HDR export never uses the SDR colour space, so that is not checked then.
    pub fn check_output(&self) -> Result<()> {
        if self.hdr_output()?.is_none() {
            self.output_space()?;
        }
        Ok(())
    }
    pub fn options(&self) -> ExportOptions {
        ExportOptions {
            quality: self.quality.clamp(1, 100),
            max_edge: if self.resize { self.long_edge } else { 0 },
            jxl_lossy: self.jxl_lossy,
        }
    }
    pub fn mime_type(&self) -> &'static str {
        self.format.mime_type()
    }
}

/// `text` as part of one file name, on any system: no folder separators or
/// characters Windows refuses, and no leading dots, so it can't name another
/// folder or a hidden file.
fn name_part(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    cleaned
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' '])
        .trim()
        .to_string()
}

/// Whether `name` is one of Windows' device names (CON, NUL, COM1…), in any case.
fn reserved(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    matches!(name.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|device| {
            // A digit, or the superscript ¹ ² ³ Windows reserves too.
            name.strip_prefix(device).is_some_and(|n| {
                let mut chars = n.chars();
                matches!(
                    (chars.next(), chars.next()),
                    (Some('0'..='9' | '¹' | '²' | '³'), None)
                )
            })
        })
}

/// "20260504" from a capture time as the catalog keeps it ("2026-05-04
/// 10:21:33") or EXIF writes it ("2026:05:04 10:21:33"); `None` without a whole
/// date.
fn date(captured: &str) -> Option<String> {
    let day = captured.get(..10)?.as_bytes();
    let separated = [4, 7].iter().all(|&i| matches!(day[i], b'-' | b':'));
    let digits: String = [&day[..4], &day[5..7], &day[8..10]]
        .concat()
        .iter()
        .map(|&b| b as char)
        .collect();
    (separated && digits.chars().all(|c| c.is_ascii_digit()) && digits != "00000000")
        .then_some(digits)
}

/// "DSC0001-2.jpg", "DSC0001-3.jpg"… for the first name not taken.
#[cfg(test)]
pub fn unique(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path.extension().unwrap_or_default().to_string_lossy();
    (2..)
        .map(|n| path.with_file_name(format!("{stem}-{n}.{extension}")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The Desktop and Pictures destinations, printed for
    /// [`desktop_and_pictures_are_the_xdg_user_folders`], which runs this
    /// test in its own process with its own home and configuration.
    #[test]
    #[ignore = "run by desktop_and_pictures_are_the_xdg_user_folders"]
    fn print_desktop_and_pictures() {
        println!("desktop={}", desktop().display());
        println!("pictures={}", pictures().display());
    }
    /// A desktop in another language, or one the user moved, names its
    /// folders in user-dirs.dirs; without that file they are under home.
    #[cfg(target_os = "linux")]
    #[test]
    fn desktop_and_pictures_are_the_xdg_user_folders() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;
        let config = home.path().join("config");
        let folders = |config: &Path| -> anyhow::Result<(String, String)> {
            let out = std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "export_settings::tests::print_desktop_and_pictures",
                ])
                .args(["--ignored", "--nocapture", "--test-threads=1"])
                .env("HOME", home.path())
                .env("XDG_CONFIG_HOME", config)
                .output()?;
            let out = String::from_utf8(out.stdout)?;
            let value = |key: &str| {
                out.lines()
                    .find_map(|line| line.split_once(key).map(|(_, value)| value))
                    .map(str::to_owned)
                    .unwrap_or_default()
            };
            Ok((value("desktop="), value("pictures=")))
        };
        let under_home = |name: &str| home.path().join(name).display().to_string();
        assert_eq!(
            folders(&config)?,
            (under_home("Desktop"), under_home("Pictures"))
        );
        std::fs::create_dir_all(&config)?;
        std::fs::write(
            config.join("user-dirs.dirs"),
            "XDG_DESKTOP_DIR=\"$HOME/Pulpit\"\nXDG_PICTURES_DIR=\"/srv/photos\"\n",
        )?;
        assert_eq!(
            folders(&config)?,
            (under_home("Pulpit"), "/srv/photos".into())
        );
        Ok(())
    }
    #[test]
    fn target_follows_destination_subfolder_and_naming() {
        let source = Path::new("/photos/2026/DSC0001.ARW");
        let mut s = ExportSettings::default();
        assert_eq!(
            s.target(source),
            Some(PathBuf::from("/photos/2026/DSC0001.jpg"))
        );
        s.subfolder = true;
        s.rename = true;
        s.custom_text = "web".into();
        s.format = Format::Tiff;
        s.uppercase = true;
        assert_eq!(
            s.target(source),
            Some(PathBuf::from("/photos/2026/Exported/DSC0001-web.TIF"))
        );
        s.destination = Destination::Folder;
        assert_eq!(s.target(source), None);
    }
    #[test]
    fn each_format_is_named_by_the_extension_it_is_read_back_from() {
        for format in [Format::Jpeg, Format::Tiff, Format::Png] {
            let path = PathBuf::from(format!("out.{}", format.extension().to_uppercase()));
            assert_eq!(Format::from_path(&path), Some(format));
        }
        assert_eq!(Format::from_path(Path::new("out.webp")), None);
    }
    #[test]
    fn a_format_from_a_later_release_keeps_the_other_settings() {
        let saved = r#"{"destination":"Folder","folder":"/exports","format":"Webp","quality":80}"#;
        let read: ExportSettings = serde_json::from_str(saved).unwrap();
        assert_eq!(read.format, Format::Jpeg);
        assert_eq!(read.folder, Some(PathBuf::from("/exports")));
        assert_eq!(read.quality, 80);
        let read: ExportSettings = serde_json::from_str(r#"{"format":"Png"}"#).unwrap();
        assert_eq!(read.format, Format::Png);
    }
    #[test]
    fn a_format_from_a_later_release_is_saved_back_until_another_is_chosen() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("export.json");
        let saved_format = |file: &Path| -> anyhow::Result<serde_json::Value> {
            let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file)?)?;
            Ok(saved["format"].clone())
        };
        std::fs::write(&file, r#"{"format":"Webp","folder":"/x"}"#)?;
        // Exported as JPEG here, and saved again by Export with Previous or a
        // watermark's rename: the later release still finds its choice.
        let mut read = ExportSettings::load_from(&file).unwrap();
        assert_eq!(read.format, Format::Jpeg);
        read.quality = 70;
        read.save_to(&file)?;
        assert_eq!(saved_format(&file)?, "Webp");
        let read = ExportSettings::load_from(&file).unwrap();
        assert_eq!((read.format, read.quality), (Format::Jpeg, 70));
        assert_eq!(read.folder, Some(PathBuf::from("/x")));
        // A format chosen here replaces it, JPEG too.
        let mut png = read;
        png.format = Format::Png;
        png.save_to(&file)?;
        assert_eq!(saved_format(&file)?, "Png");
        std::fs::write(&file, r#"{"format":"Webp"}"#)?;
        let mut jpeg = ExportSettings::load_from(&file).unwrap();
        jpeg.choose_format(Format::Jpeg);
        jpeg.save_to(&file)?;
        assert_eq!(saved_format(&file)?, "Jpeg");
        // The formats known here are saved as they were read.
        for format in Format::ALL {
            let name = serde_json::to_value(format)?;
            std::fs::write(&file, format!(r#"{{"format":{name}}}"#))?;
            ExportSettings::load_from(&file).unwrap().save_to(&file)?;
            assert_eq!(saved_format(&file)?, name);
        }
        Ok(())
    }
    /// JPEG XL exports were lossless before they could be lossy, so settings saved
    /// then still export lossless; new settings start lossy, at the JPEG quality.
    #[test]
    fn jpeg_xl_settings_saved_before_lossy_ones_stay_lossless() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("export.json");
        std::fs::write(&file, r#"{"format":"Jxl","quality":80,"newer_flag":1}"#)?;
        let mut read = ExportSettings::load_from(&file).unwrap();
        assert!(!read.jxl_lossy);
        assert!(!read.options().jxl_lossy);
        assert_eq!(read.options().quality, 80);
        read.save_to(&file)?;
        let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
        assert_eq!(written["jxl_lossy"], false);
        assert_eq!(written["newer_flag"], 1);
        read.jxl_lossy = true;
        read.save_to(&file)?;
        let lossy = ExportSettings::load_from(&file).unwrap();
        assert!(lossy.jxl_lossy && lossy.options().jxl_lossy);
        let fresh = ExportSettings::default();
        assert!(fresh.jxl_lossy && fresh.options().jxl_lossy);
        // An edit's own export options (saved in the catalog) never carry it, and read
        // as lossless, as the command line exports.
        let options: ExportOptions = serde_json::from_str(r#"{"quality":80,"max_edge":0}"#)?;
        assert!(!options.jxl_lossy);
        assert_eq!(
            serde_json::to_value(&options)?,
            serde_json::json!({"quality": 80, "max_edge": 0})
        );
        Ok(())
    }
    #[test]
    fn settings_a_later_release_added_are_saved_back() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("export.json");
        let saved = |file: &Path| -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::from_str(&std::fs::read_to_string(file)?)?)
        };
        std::fs::write(
            &file,
            r#"{"folder":"/x","quality":80,"sharpen":{"amount":2},"newer_flag":true}"#,
        )?;
        let mut read = ExportSettings::load_from(&file).unwrap();
        assert_eq!(read.folder, Some(PathBuf::from("/x")));
        assert_eq!(read.quality, 80);
        // Saved again by an export here: the later release still finds its
        // settings, and what was changed here is saved.
        read.quality = 70;
        read.save_to(&file)?;
        let written = saved(&file)?;
        assert_eq!(written["sharpen"], serde_json::json!({"amount": 2}));
        assert_eq!(written["newer_flag"], true);
        assert_eq!(written["folder"], "/x");
        assert_eq!(written["quality"], 70);
        assert_eq!(ExportSettings::load_from(&file).unwrap(), read);
        // With a format only that release knows, both are saved back.
        std::fs::write(&file, r#"{"format":"Webp","newer_flag":true}"#)?;
        ExportSettings::load_from(&file).unwrap().save_to(&file)?;
        let written = saved(&file)?;
        assert_eq!(written["format"], "Webp");
        assert_eq!(written["newer_flag"], true);
        Ok(())
    }
    /// Settings saved before the choice export sRGB; each space reads back as saved.
    #[test]
    fn the_color_space_is_srgb_unless_chosen_and_reads_back() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("export.json");
        std::fs::write(&file, r#"{"format":"Tiff","folder":"/x"}"#)?;
        let read = ExportSettings::load_from(&file).unwrap();
        assert_eq!(read.color_space, OutputSpace::Srgb);
        assert_eq!(read.output_space()?, OutputSpace::Srgb);
        for space in OutputSpace::ALL {
            let mut settings = read.clone();
            settings.choose_color_space(space);
            settings.save_to(&file)?;
            let back = ExportSettings::load_from(&file).unwrap();
            assert_eq!(back.output_space()?, space);
            assert_eq!(back, settings);
        }
        Ok(())
    }
    /// ProPhoto RGB is written only as 16-bit TIFF: with JPEG or PNG it is refused,
    /// before anything renders. A JPEG XL is sRGB.
    #[test]
    fn prophoto_needs_tiff_and_the_others_take_any_format() {
        for format in Format::ALL {
            for space in OutputSpace::ALL {
                let mut s = ExportSettings::default();
                s.choose_format(format);
                s.choose_color_space(space);
                let allowed = if format == Format::Jxl {
                    space == OutputSpace::Srgb
                } else {
                    space != OutputSpace::ProPhoto || format == Format::Tiff
                };
                assert_eq!(s.output_space().is_ok(), allowed, "{format:?} {space:?}");
                assert_eq!(format.supports(space).is_ok(), allowed);
            }
        }
        let mut s = ExportSettings::default();
        s.choose_color_space(OutputSpace::ProPhoto);
        let error = s.output_space().unwrap_err().to_string();
        assert!(
            error.contains("ProPhoto RGB") && error.contains("TIFF"),
            "{error}"
        );
    }
    /// An HDR export only a later release knows, or one with a field only it knows,
    /// is kept and saved back and refused, never exported as SDR, until HDR export
    /// is chosen here.
    #[test]
    fn an_hdr_export_from_a_later_release_is_kept_and_refused() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("export.json");
        let saved = |file: &Path| -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::from_str(&std::fs::read_to_string(file)?)?)
        };
        for later in [
            r#"{"gamut":"rec2020","transfer":"future"}"#,
            r#"{"gamut":"rec2020","transfer":"pq","peak":1000}"#,
        ] {
            std::fs::write(&file, format!(r#"{{"format":"Tiff","hdr":{later}}}"#))?;
            let mut read = ExportSettings::load_from(&file).unwrap();
            assert!(read.hdr.is_none());
            let error = read.hdr_output().unwrap_err().to_string();
            assert!(error.contains("later version"), "{error}");
            read.quality = 70;
            read.save_to(&file)?;
            let back: serde_json::Value = serde_json::from_str(later)?;
            assert_eq!(saved(&file)?["hdr"], back);
            read.choose_hdr(None);
            assert!(read.hdr_output()?.is_none());
            read.save_to(&file)?;
            assert!(saved(&file)?.get("hdr").is_none());
        }
        // A null HDR export is none, not a later one.
        std::fs::write(&file, r#"{"format":"Jpeg","hdr":null}"#)?;
        let read = ExportSettings::load_from(&file).unwrap();
        assert_eq!(read.hdr_output()?, None);
        // A gain map only in JPEG, over an sRGB (Rec. 709) base; HDR pixels never in
        // JPEG.
        use crate::color::hdr_output::{HdrGamut, HdrOutput};
        let gain = HdrOutput {
            gamut: HdrGamut::Rec709,
            gain_map: true,
            ..Default::default()
        };
        assert!(Format::check_hdr(Format::Jpeg, gain).is_ok());
        assert!(Format::check_hdr(Format::Tiff, gain).is_err());
        assert!(Format::check_hdr(Format::Jxl, gain).is_ok());
        assert!(
            Format::check_hdr(
                Format::Jpeg,
                HdrOutput {
                    gamut: HdrGamut::Rec2020,
                    ..gain
                }
            )
            .is_err()
        );
        assert!(Format::check_hdr(Format::Jpeg, HdrOutput::default()).is_err());
        // One this release knows round-trips.
        let mut known = ExportSettings::default();
        known.choose_format(Format::Tiff);
        known.choose_hdr(Some(Default::default()));
        known.save_to(&file)?;
        let read = ExportSettings::load_from(&file).unwrap();
        assert_eq!(read.hdr_output()?, Some(Default::default()));
        Ok(())
    }
    /// A space only a later release knows is kept and saved back, as a later
    /// format is, but never exported as another: an export refuses it until a space
    /// is chosen here.
    #[test]
    fn a_color_space_from_a_later_release_is_kept_and_refused() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("export.json");
        let saved = |file: &Path| -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::from_str(&std::fs::read_to_string(file)?)?)
        };
        std::fs::write(&file, r#"{"color_space":"DisplayP3","quality":80}"#)?;
        let mut read = ExportSettings::load_from(&file).unwrap();
        assert_eq!(read.quality, 80);
        assert!(read.later_color_space().is_some());
        let error = read.output_space().unwrap_err().to_string();
        assert!(
            error.contains("DisplayP3") && error.contains("color space"),
            "{error}"
        );
        read.quality = 70;
        read.save_to(&file)?;
        assert_eq!(saved(&file)?["color_space"], "DisplayP3");
        assert_eq!(saved(&file)?["quality"], 70);
        let mut read = ExportSettings::load_from(&file).unwrap();
        assert!(read.output_space().is_err());
        read.choose_color_space(OutputSpace::AdobeRgb);
        assert_eq!(read.output_space()?, OutputSpace::AdobeRgb);
        read.save_to(&file)?;
        assert_eq!(saved(&file)?["color_space"], "AdobeRgb");
        Ok(())
    }
    #[test]
    fn settings_this_release_knows_are_saved_as_they_were() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("export.json");
        let mut known = vec![
            "destination",
            "folder",
            "subfolder",
            "subfolder_name",
            "existing",
            "rename",
            "custom_text",
            "naming",
            "start_number",
            "uppercase",
            "format",
            "color_space",
            "quality",
            "jxl_lossy",
            "resize",
            "long_edge",
            "ppi",
            "capture",
            "location",
            "develop",
            "descriptive",
            "include",
            "remove_location",
            "watermark",
            "watermark_name",
            "after_export",
        ];
        known.sort_unstable();
        let keys = |value: &serde_json::Value| -> Vec<String> {
            let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort_unstable();
            keys
        };
        assert_eq!(
            keys(&serde_json::to_value(ExportSettings::default())?),
            known
        );
        std::fs::write(&file, r#"{"quality":80,"folder":"/x"}"#)?;
        ExportSettings::load_from(&file).unwrap().save_to(&file)?;
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
        assert_eq!(keys(&saved), known);
        assert_eq!(
            (saved["quality"].clone(), saved["folder"].clone()),
            (80.into(), "/x".into())
        );
        Ok(())
    }
    #[test]
    fn every_export_extension_and_mime_type_is_what_it_was() {
        for (name, format) in [
            ("a.jpeg", Format::Jpeg),
            ("a.JPEG", Format::Jpeg),
            ("a.tiff", Format::Tiff),
            ("a.Tiff", Format::Tiff),
            ("a.png", Format::Png),
            ("a.JXL", Format::Jxl),
        ] {
            assert_eq!(Format::from_path(Path::new(name)), Some(format), "{name}");
        }
        assert_eq!(
            Format::ALL.map(Format::mime_type),
            ["image/jpeg", "image/tiff", "image/png", "image/jxl"]
        );
        assert_eq!(
            Format::extensions_listed(),
            ".jpg, .jpeg, .tif, .tiff, .png or .jxl"
        );
    }
    #[test]
    fn rename_templates_name_each_photo_as_lightrooms_do() {
        let source = Path::new("/photos/DSC0001.ARW");
        let mut s = ExportSettings {
            custom_text: "Concert".into(),
            start_number: 7,
            ..Default::default()
        };
        let fields = NameFields::captured(Some("2026-05-04 10:21:33.000".into()));
        let at = NameContext {
            index: 2,
            total: 30,
            fields: &fields,
        };
        let name = |s: &ExportSettings, naming| {
            let s = ExportSettings {
                naming: Some(naming),
                ..s.clone()
            };
            s.file_name_for(source, &at).0
        };
        assert_eq!(name(&s, Naming::Filename), "DSC0001.jpg");
        assert_eq!(name(&s, Naming::FilenameText), "DSC0001-Concert.jpg");
        assert_eq!(name(&s, Naming::FilenameSequence), "DSC0001-9.jpg");
        assert_eq!(name(&s, Naming::CustomName), "Concert.jpg");
        assert_eq!(name(&s, Naming::CustomNameSequence), "Concert-9.jpg");
        assert_eq!(name(&s, Naming::CustomNameOf), "Concert (9 of 30).jpg");
        assert_eq!(name(&s, Naming::DateFilename), "20260504-DSC0001.jpg");
        // EXIF's form of the date too.
        let exif_fields = NameFields::captured(Some("2026:05:04 10:21:33".into()));
        let exif = NameContext {
            fields: &exif_fields,
            ..at
        };
        s.naming = Some(Naming::DateFilename);
        assert_eq!(s.file_name_for(source, &exif).0, "20260504-DSC0001.jpg");
        // No capture date: "undated", said, never the file's modified date.
        let (name, note) = s.file_name_for(source, &NameContext::ONE);
        assert_eq!(name, "undated-DSC0001.jpg");
        assert_eq!(note.len(), 1);
        // Custom text names one file, never another folder.
        s.naming = Some(Naming::CustomName);
        for text in ["/tmp/final", "../final", "a\\b:c"] {
            s.custom_text = text.into();
            let name = s.file_name_for(source, &at).0;
            assert!(
                !name.contains(['/', '\\', ':']) && !name.starts_with('.'),
                "{name}"
            );
        }
        // Windows' device names are not left as they are.
        s.custom_text = "con".into();
        assert_eq!(s.file_name_for(source, &at).0, "con_.jpg");
        s.custom_text = "CON.txt".into();
        assert_eq!(s.file_name_for(source, &at).0, "CON_.txt.jpg");
        s.custom_text = "lpt².txt".into();
        assert_eq!(s.file_name_for(source, &at).0, "lpt²_.txt.jpg");
        // An empty custom name is the file's own.
        s.custom_text = " ".into();
        s.naming = Some(Naming::CustomName);
        assert_eq!(s.file_name_for(source, &at).0, "DSC0001.jpg");
    }
    #[test]
    fn a_template_numbers_from_its_start_with_zeros_in_front() {
        let source = Path::new("/photos/DSC0001.ARW");
        let template = |padding| NameTemplate {
            naming: Naming::CustomNameSequence,
            text: "Concert".into(),
            start: 7,
            padding,
            tokens: None,
        };
        let at = |index| NameContext {
            index,
            total: 200,
            ..NameContext::ONE
        };
        assert_eq!(template(3).stem(source, &at(2)).0, "Concert-009");
        assert_eq!(template(0).stem(source, &at(2)).0, "Concert-9");
        assert_eq!(template(1).stem(source, &at(2)).0, "Concert-9");
        assert_eq!(template(2).stem(source, &at(193)).0, "Concert-200");
        let of = NameTemplate {
            naming: Naming::CustomNameOf,
            ..template(3)
        };
        assert_eq!(of.stem(source, &at(0)).0, "Concert (007 of 200)");
        // An export names its files as before, with no zeros added.
        let s = ExportSettings {
            naming: Some(Naming::CustomNameSequence),
            custom_text: "Concert".into(),
            start_number: 7,
            ..Default::default()
        };
        assert_eq!(s.name_template(), template(0));
    }
    #[test]
    fn settings_saved_before_the_templates_keep_their_names() {
        let renamed: ExportSettings =
            serde_json::from_str(r#"{"rename": true, "custom_text": "web"}"#).unwrap();
        assert_eq!(renamed.naming(), Naming::FilenameText);
        assert_eq!(
            renamed.file_name(Path::new("/p/DSC0001.ARW")),
            "DSC0001-web.jpg"
        );
        let plain: ExportSettings = serde_json::from_str(r#"{"custom_text": "web"}"#).unwrap();
        assert_eq!(plain.naming(), Naming::Filename);
        assert_eq!(plain.start_number, 1);
    }
    #[test]
    fn older_settings_files_keep_the_other_defaults() {
        let old: ExportSettings = serde_json::from_str("{\"quality\": 80}").unwrap();
        assert_eq!(old.quality, 80);
        assert!(old.capture && old.develop);
        // Settings saved before the Include popup keep their switches.
        assert_eq!(old.include, Include::Custom);
    }
    #[test]
    fn unique_names_count_up_from_two() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("a.jpg");
        std::fs::write(&path, b"")?;
        std::fs::write(dir.path().join("a-2.jpg"), b"")?;
        assert_eq!(unique(&path), dir.path().join("a-3.jpg"));
        Ok(())
    }
}
