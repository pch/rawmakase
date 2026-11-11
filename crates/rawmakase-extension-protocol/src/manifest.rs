//! `rawmakase-extension.toml`: what an extension is and what it contributes.
//!
//! Unknown keys are an error everywhere in the manifest, tables included, so a
//! typo can't silently disable a feature and a contribution table this version
//! doesn't know (`[[filter]]`, `[[importer]]`, …) is refused rather than
//! ignored. [`Manifest::parse`] reads and validates; the JSON Schema
//! ([`schema`]) is committed as `docs/extensions/manifest.schema.json`.
use crate::controls::Choice;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The manifest's file name, at the root of a package or an extension's folder.
pub const FILE: &str = "rawmakase-extension.toml";

/// The manifest format this version reads, the `api` key. A manifest with a
/// later one was written for a later RAWmakase, which knows tables this one
/// doesn't.
pub const API: u32 = 2;

/// The first `api` that reads `[[mask-generator]]`.
pub const MASK_GENERATOR_API: u32 = 2;

/// The bundled extension that may claim the provenance Select Subject and
/// Select Sky saved before extensions existed.
pub const AI_MASKS: &str = "rawmakase.ai-masks";

/// The provenance values saved masks already carry, which only [`AI_MASKS`]
/// may claim.
pub const LEGACY_PROVENANCE: [&str; 2] = ["subject", "sky"];

/// A parsed manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "rawmakase-extension.toml")]
pub struct Manifest {
    /// What the extension is.
    pub extension: Extension,
    /// What it reaches beyond the app, shown before installing.
    #[serde(default)]
    pub access: Access,
    /// The program behind its commands, panels and control adapters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<Program>,
    /// Settings, shown on the extension's page in Preferences.
    #[serde(default, rename = "setting", skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<Setting>,
    /// Commands, saved as `<extension>/<command>` in the command registry.
    #[serde(default, rename = "command", skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<Command>,
    /// Panels of host-drawn controls the program fills.
    #[serde(default, rename = "panel", skip_serializing_if = "Vec::is_empty")]
    pub panels: Vec<Panel>,
    /// Adapters translating a device or a protocol into commands.
    #[serde(
        default,
        rename = "control-adapter",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub control_adapters: Vec<ControlAdapter>,
    /// Generators of raster masks, from `api = 2`.
    #[serde(
        default,
        rename = "mask-generator",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub mask_generators: Vec<MaskGenerator>,
}

/// The `[extension]` table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Extension {
    /// Reverse-DNS (`dev.jane.analog`), permanent: edits and settings store it.
    /// Official extensions use `rawmakase.*`.
    #[schemars(regex(pattern = r"^[a-z0-9][a-z0-9-]*(\.[a-z0-9][a-z0-9-]*)+$"))]
    pub id: String,
    /// The name shown in Preferences.
    pub name: String,
    /// The extension's version, semver.
    #[schemars(with = "String")]
    pub version: semver::Version,
    /// The RAWmakase versions it supports, as a semver range (`>=0.6, <0.7`);
    /// refused outside them.
    #[schemars(with = "String")]
    pub rawmakase: semver::VersionReq,
    /// The manifest format it was written for; this version reads [`API`].
    #[schemars(range(min = 1))]
    pub api: u32,
    /// Who wrote it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// Its license, an SPDX expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Its source repository, `https://github.com/<owner>/<repo>` for updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// One or two sentences for its page in Preferences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A PNG in the package, relative to its root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// The `[access]` table: what the extension declares it reaches. Shown before
/// installing and when an update asks for more; not enforced, since an
/// extension runs with the user's permissions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Access {
    /// Whether it uses the network.
    #[serde(default)]
    pub network: bool,
    /// Which files it reads and writes.
    #[serde(default)]
    pub files: Files,
    /// Kinds of devices it talks to, for example `midi`, `usb`, `camera`.
    #[serde(default)]
    pub devices: Vec<String>,
}
impl Access {
    /// What `self` asks for that `before` didn't: an update that asks for more
    /// says so before it is installed.
    pub fn more_than(&self, before: &Access) -> Vec<String> {
        let mut more = Vec::new();
        if self.network && !before.network {
            more.push("the network".to_string());
        }
        if self.files == Files::Any && before.files != Files::Any {
            more.push("any file".to_string());
        }
        for device in &self.devices {
            if !before.devices.contains(device) {
                more.push(format!("{device} devices"));
            }
        }
        more
    }
}

/// Which files an extension declares it reaches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Files {
    /// The photos the app hands it.
    #[default]
    Photos,
    /// Any file the user can read.
    Any,
}

/// The `[program]` table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Program {
    /// The executable, relative to the package's root, with `/` between
    /// folders. On Windows `.exe` is added when the path has no extension.
    pub path: String,
    /// When it starts.
    #[serde(default)]
    pub start: Start,
}

/// When a program starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Start {
    /// At its first request, and stopped after a while with nothing to do.
    #[default]
    OnDemand,
    /// With the app, running until it quits.
    WithApp,
}

/// A `[[setting]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    /// Unique among the extension's settings.
    #[schemars(regex(pattern = r"^[a-z0-9][a-z0-9-]*$"))]
    pub id: String,
    /// The label shown beside it; the id without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A line of help under it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// How it is edited.
    pub control: SettingControl,
    /// Its value until the user changes it, of the control's type.
    pub default: Value,
    /// A slider's step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// A slider's or a number's unit, for example `%`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// How a setting is edited: a host-drawn control
/// ([`crate::controls::ControlKind`]) without its value, which is the
/// setting's. Written `control = "toggle"` or `control = { slider = [0, 100] }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum SettingControl {
    /// A number in `[min, max]`.
    Slider([f64; 2]),
    /// A number typed in, in `[min, max]`.
    Number([f64; 2]),
    /// An angle in degrees.
    Angle,
    /// On or off.
    Toggle,
    /// One of a list, as a drop-down.
    Choice(Vec<String>),
    /// One of a short list, as buttons.
    Segmented(Vec<String>),
    /// Text.
    Text,
    /// An sRGB colour, `[r, g, b]`, 0 to 1.
    Color,
    /// A file chosen in the system's dialog.
    File,
    /// A folder chosen in the system's dialog.
    Folder,
}
impl SettingControl {
    /// Whether `value` is one this control holds.
    pub fn accepts(&self, value: &Value) -> bool {
        let in_range = |[min, max]: [f64; 2]| value.as_f64().is_some_and(|v| min <= v && v <= max);
        match self {
            Self::Slider(range) | Self::Number(range) => in_range(*range),
            Self::Angle => value.as_f64().is_some(),
            Self::Toggle => value.is_boolean(),
            Self::Choice(options) | Self::Segmented(options) => value
                .as_str()
                .is_some_and(|v| options.iter().any(|o| o == v)),
            Self::Text | Self::File | Self::Folder => value.is_string(),
            Self::Color => value.as_array().is_some_and(|c| {
                c.len() == 3
                    && c.iter()
                        .all(|c| c.as_f64().is_some_and(|c| (0.0..=1.0).contains(&c)))
            }),
        }
    }
    /// The options of a choice, as the host-drawn control lists them.
    pub fn choices(&self) -> Vec<Choice> {
        match self {
            Self::Choice(options) | Self::Segmented(options) => options
                .iter()
                .map(|o| Choice {
                    id: o.clone(),
                    title: o.clone(),
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// A `[[command]]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Command {
    /// Unique among the extension's commands; registered as
    /// `<extension>/<id>`.
    #[schemars(regex(pattern = r"^[a-z0-9][a-z0-9-]*$"))]
    pub id: String,
    /// The title in menus, the command palette and Preferences > Shortcuts.
    pub title: String,
    /// The fixed slots it is shown in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub places: Vec<Place>,
    /// Its default key, in the key map's notation (`Ctrl+Alt+F`); the user's
    /// key map overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// What it acts on; the app refuses it when that isn't there.
    #[serde(default)]
    pub applies_to: AppliesTo,
    /// Viewer input it collects before running.
    #[serde(default)]
    pub input: Input,
}

/// A fixed slot of the app's interface a command may be placed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Place {
    /// The Library's photo menu.
    #[serde(rename = "library.photo-menu")]
    LibraryPhotoMenu,
    /// The Library's toolbar.
    #[serde(rename = "library.toolbar")]
    LibraryToolbar,
    /// The Library's Auto menu.
    #[serde(rename = "library.auto")]
    LibraryAuto,
    /// Develop's Auto menu.
    #[serde(rename = "develop.auto")]
    DevelopAuto,
    /// Develop's tool strip.
    #[serde(rename = "develop.tool-strip")]
    DevelopToolStrip,
    /// File > Import.
    #[serde(rename = "file.import")]
    FileImport,
    /// File > Export.
    #[serde(rename = "file.export")]
    FileExport,
    /// The extension's own menu under Extensions.
    #[serde(rename = "extensions.menu")]
    ExtensionsMenu,
}

/// What a command acts on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AppliesTo {
    /// Nothing in particular.
    #[default]
    None,
    /// The photo open in Develop.
    Photo,
    /// The Library's selection.
    Selection,
}

/// Viewer input a command collects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Input {
    /// None.
    #[default]
    None,
    /// One click.
    Point,
    /// Positive and negative clicks.
    Points,
    /// One dragged box.
    Box,
    /// Clicks and an optional box, refined together.
    Prompt,
    /// A brushed coverage raster.
    Brush,
}

/// A `[[panel]]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Panel {
    /// Unique among the extension's panels.
    #[schemars(regex(pattern = r"^[a-z0-9][a-z0-9-]*$"))]
    pub id: String,
    /// The panel's title.
    pub title: String,
    /// Where it is shown.
    pub place: PanelPlace,
}

/// Where a panel is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PanelPlace {
    /// Develop's right column.
    Develop,
    /// The Library's right column.
    Library,
    /// A window of the extension's own.
    Window,
    /// The extension's page in Preferences > Extensions.
    Preferences,
}

/// A `[[control-adapter]]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlAdapter {
    /// Unique among the extension's adapters; control requests name it as
    /// `<extension>/<id>`.
    #[schemars(regex(pattern = r"^[a-z0-9][a-z0-9-]*$"))]
    pub id: String,
    /// The name shown in Preferences; the id without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Who starts its program.
    #[serde(default)]
    pub launch: Launch,
}

/// Who starts a control adapter's program.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Launch {
    /// The app, as the extension's program (MIDI).
    #[default]
    App,
    /// Something else, such as an AI agent starting an MCP server: the
    /// program isn't supervised by the app and reaches it through the control
    /// socket.
    External,
}

/// A `[[mask-generator]]`: the program turns the photo, and the user's viewer
/// input, into a raster mask's coverage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct MaskGenerator {
    /// Unique among the extension's mask generators.
    #[schemars(regex(pattern = r"^[a-z0-9][a-z0-9-]*$"))]
    pub id: String,
    /// The viewer input it takes, each kind once: `none` (automatic) and
    /// `prompt` (aiming and refining with clicks and a box). Other kinds are
    /// refused until a version that collects them for mask generators.
    pub input: Vec<Input>,
    /// What saved masks record as their source, so a mask finds its generator
    /// again to be refined or regenerated: `<extension>/<id>` unless stated.
    /// Only `rawmakase.ai-masks` may state anything else, and only `subject`
    /// or `sky`, the values saved before extensions existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
    /// The line shown while the user aims with `prompt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_hint: Option<String>,
    /// What the mask drawer offers to make a new mask with this generator.
    #[serde(default, rename = "tile", skip_serializing_if = "Vec::is_empty")]
    pub tiles: Vec<Tile>,
}
impl MaskGenerator {
    /// The provenance saved masks record: the stated one, or
    /// `<extension>/<id>`.
    pub fn provenance(&self, extension: &str) -> String {
        self.provenance
            .clone()
            .unwrap_or_else(|| format!("{extension}/{}", self.id))
    }
}

/// A `[[mask-generator.tile]]`: a shortcut in the mask drawer that asks its
/// generator for a new mask component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tile {
    /// The tile's name, for example `Background`.
    pub name: String,
    /// Whether the new component is inverted: Background is an inverted
    /// Subject.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invert: bool,
}

/// Why a manifest was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid(pub String);
impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Invalid {}

impl Manifest {
    /// Reads and validates a manifest's text.
    pub fn parse(text: &str) -> Result<Self, Invalid> {
        let manifest: Self = toml::from_str(text).map_err(|e| Invalid(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// The contributions it declares, as `<table>/<id>`, which `hello` sends.
    pub fn contributions(&self) -> Vec<String> {
        let commands = self.commands.iter().map(|c| format!("command/{}", c.id));
        let panels = self.panels.iter().map(|p| format!("panel/{}", p.id));
        let adapters = self
            .control_adapters
            .iter()
            .map(|a| format!("control-adapter/{}", a.id));
        let generators = self
            .mask_generators
            .iter()
            .map(|g| format!("mask-generator/{}", g.id));
        commands
            .chain(panels)
            .chain(adapters)
            .chain(generators)
            .collect()
    }

    /// Whether the app's program supervision starts it: a program serving
    /// anything other than externally launched adapters.
    pub fn runs_in_app(&self) -> bool {
        self.program.is_some()
            && (!self.commands.is_empty()
                || !self.panels.is_empty()
                || !self.mask_generators.is_empty()
                || self
                    .control_adapters
                    .iter()
                    .any(|a| a.launch == Launch::App))
    }

    /// The checks the types can't make: id syntax, unique ids, defaults that
    /// fit their controls, a program for every contribution that needs one,
    /// paths inside the package.
    pub fn validate(&self) -> Result<(), Invalid> {
        let e = &self.extension;
        let fail = |why: String| Err(Invalid(why));
        if !is_extension_id(&e.id) {
            return fail(format!(
                "extension.id {:?} must be reverse-DNS: lowercase letters, digits and hyphens, \
                 in two or more parts separated by dots",
                e.id
            ));
        }
        if e.api > API {
            return fail(format!(
                "extension.api is {}, for a newer RAWmakase (this one reads {API})",
                e.api
            ));
        }
        if e.api == 0 {
            return fail("extension.api starts at 1".into());
        }
        if e.name.trim().is_empty() {
            return fail("extension.name is empty".into());
        }
        if let Some(icon) = &e.icon {
            check_path("extension.icon", icon)?;
        }
        if let Some(program) = &self.program {
            check_path("program.path", &program.path)?;
        }
        let tables = [
            (
                "setting",
                self.settings.iter().map(|s| &s.id).collect::<Vec<_>>(),
            ),
            ("command", self.commands.iter().map(|c| &c.id).collect()),
            ("panel", self.panels.iter().map(|p| &p.id).collect()),
            (
                "control-adapter",
                self.control_adapters.iter().map(|a| &a.id).collect(),
            ),
            (
                "mask-generator",
                self.mask_generators.iter().map(|g| &g.id).collect(),
            ),
        ];
        for (table, ids) in tables {
            let mut seen = std::collections::BTreeSet::new();
            for id in ids {
                if !is_local_id(id) {
                    return fail(format!(
                        "{table} id {id:?} must be lowercase letters, digits and hyphens"
                    ));
                }
                if !seen.insert(id) {
                    return fail(format!("{table} id {id:?} is used twice"));
                }
            }
        }
        for setting in &self.settings {
            let id = &setting.id;
            match &setting.control {
                SettingControl::Slider([min, max]) | SettingControl::Number([min, max])
                    if !(min.is_finite() && max.is_finite() && min < max) =>
                {
                    return fail(format!("setting {id}: its range is empty"));
                }
                SettingControl::Choice(options) | SettingControl::Segmented(options)
                    if options.is_empty() =>
                {
                    return fail(format!("setting {id}: it has no options"));
                }
                _ => {}
            }
            if !setting.control.accepts(&setting.default) {
                return fail(format!(
                    "setting {id}: its default {} doesn't fit its control",
                    setting.default
                ));
            }
            if setting.step.is_some_and(|s| !(s.is_finite() && s > 0.0)) {
                return fail(format!("setting {id}: its step is not positive"));
            }
        }
        for command in &self.commands {
            if command.title.trim().is_empty() {
                return fail(format!("command {}: its title is empty", command.id));
            }
            if command.key.as_ref().is_some_and(|k| k.trim().is_empty()) {
                return fail(format!("command {}: its key is empty", command.id));
            }
        }
        self.validate_mask_generators()?;
        let needs_program = !self.commands.is_empty()
            || !self.panels.is_empty()
            || !self.control_adapters.is_empty()
            || !self.mask_generators.is_empty();
        if needs_program && self.program.is_none() {
            return fail(
                "commands, panels, control adapters and mask generators need a [program] to run them"
                    .into(),
            );
        }
        Ok(())
    }

    /// The `[[mask-generator]]` rules: the `api` that reads them, the input
    /// kinds built so far, and one generator per provenance, so a saved mask
    /// finds exactly one generator.
    fn validate_mask_generators(&self) -> Result<(), Invalid> {
        let e = &self.extension;
        let fail = |why: String| Err(Invalid(why));
        if self.mask_generators.is_empty() {
            return Ok(());
        }
        if e.api < MASK_GENERATOR_API {
            return fail(format!(
                "[[mask-generator]] needs extension.api = {MASK_GENERATOR_API} or later (this one says {})",
                e.api
            ));
        }
        let mut provenances = std::collections::BTreeSet::new();
        for generator in &self.mask_generators {
            let id = &generator.id;
            if generator.input.is_empty() {
                return fail(format!("mask-generator {id}: its input is empty"));
            }
            for (i, input) in generator.input.iter().enumerate() {
                if !matches!(input, Input::None | Input::Prompt) {
                    return fail(format!(
                        "mask-generator {id}: input {input:?} isn't supported yet; use none or prompt"
                    ));
                }
                if generator.input[..i].contains(input) {
                    return fail(format!(
                        "mask-generator {id}: input {input:?} is listed twice"
                    ));
                }
            }
            let own = format!("{}/{id}", e.id);
            let provenance = generator.provenance(&e.id);
            let legacy = LEGACY_PROVENANCE.contains(&provenance.as_str()) && e.id == AI_MASKS;
            if provenance != own && !legacy {
                return fail(format!(
                    "mask-generator {id}: its provenance {provenance:?} must be {own:?}"
                ));
            }
            if !provenances.insert(provenance.clone()) {
                return fail(format!(
                    "mask-generator {id}: provenance {provenance:?} is used twice"
                ));
            }
            if generator
                .prompt_hint
                .as_ref()
                .is_some_and(|h| h.trim().is_empty())
            {
                return fail(format!("mask-generator {id}: its prompt-hint is empty"));
            }
            for tile in &generator.tiles {
                if tile.name.trim().is_empty() {
                    return fail(format!("mask-generator {id}: a tile's name is empty"));
                }
            }
        }
        Ok(())
    }
}

/// `dev.jane.analog`: two or more parts of lowercase letters, digits and
/// hyphens, each starting with a letter or a digit.
pub fn is_extension_id(id: &str) -> bool {
    id.len() <= 128 && id.split('.').count() >= 2 && id.split('.').all(is_local_id)
}

/// A contribution's id within its extension: lowercase letters, digits and
/// hyphens, starting with a letter or a digit.
pub fn is_local_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 64
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A path inside the package: relative, `/` between plain names, no `..`.
fn check_path(key: &str, path: &str) -> Result<(), Invalid> {
    if is_package_path(path) {
        Ok(())
    } else {
        Err(Invalid(format!(
            "{key} {path:?} must be a relative path inside the package, with / between plain names"
        )))
    }
}

/// Whether `path` names a file inside a package: relative, `/` between names,
/// none empty, `.` or `..`, and no backslash, colon or control character, so it
/// means the same on every platform.
///
/// Names Windows would read as something else are refused on every platform,
/// since a package built on Linux installs on Windows too: Win32 strips trailing
/// dots and spaces from a name (`readme.` is `readme`, `.. ` is `..`), and opens
/// a DOS device for a name whose stem is one (`NUL.txt`, `COM1`), whatever the
/// folder.
pub fn is_package_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 512
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with(['.', ' '])
                && !is_dos_device(part)
                && !part
                    .chars()
                    .any(|c| c == '\\' || c == ':' || c.is_control())
        })
}

/// Whether Windows opens a device for this name: its stem, before the first dot
/// and with trailing spaces removed, is a reserved device name in any case. The
/// digits of `COM` and `LPT` include the superscripts ¹ ² ³, which Windows
/// counts as digits there.
fn is_dos_device(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).trim_end_matches(' ');
    let upper = stem.to_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => ["COM", "LPT"].iter().any(|prefix| {
            upper.strip_prefix(prefix).is_some_and(|rest| {
                let mut chars = rest.chars();
                matches!(
                    (chars.next(), chars.next()),
                    (Some('1'..='9' | '¹' | '²' | '³'), None)
                )
            })
        }),
    }
}

/// The manifest's JSON Schema, as committed in
/// `docs/extensions/manifest.schema.json`.
pub fn schema() -> Value {
    serde_json::to_value(schemars::schema_for!(Manifest)).expect("a schema is JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
[extension]
id = "dev.jane.analog"
name = "Analog Toolkit"
version = "1.2.0"
rawmakase = ">=0.6, <0.7"
api = 1
authors = ["Jane Doe"]
license = "MIT"
repository = "https://github.com/jane/rawmakase-analog"
description = "Film-stock Auto looks."

[access]
network = false
files = "photos"
devices = []

[program]
path = "bin/analog"
start = "on-demand"

[[setting]]
id = "default-stock"
control = { choice = ["Tri-X", "HP5", "Portra 400"] }
default = "Tri-X"

[[setting]]
id = "strength"
control = { slider = [0, 100] }
default = 50
unit = "%"

[[setting]]
id = "keep"
control = "toggle"
default = true

[[command]]
id = "auto-film-look"
title = "Auto Film Look"
places = ["library.photo-menu", "library.auto", "develop.auto"]
key = "Ctrl+Alt+F"
applies-to = "selection"
input = "none"

[[panel]]
id = "looks"
title = "Film Looks"
place = "develop"

[[control-adapter]]
id = "mcp"
launch = "external"
"#;

    #[test]
    fn the_example_reads_and_round_trips() {
        let manifest = Manifest::parse(EXAMPLE).unwrap();
        assert_eq!(manifest.extension.version, semver::Version::new(1, 2, 0));
        assert!(
            manifest
                .extension
                .rawmakase
                .matches(&semver::Version::new(0, 6, 3))
        );
        assert_eq!(manifest.commands[0].applies_to, AppliesTo::Selection);
        assert_eq!(manifest.control_adapters[0].launch, Launch::External);
        assert_eq!(
            manifest.contributions(),
            [
                "command/auto-film-look",
                "panel/looks",
                "control-adapter/mcp"
            ]
        );
        let written = toml::to_string(&manifest).unwrap();
        assert_eq!(Manifest::parse(&written).unwrap(), manifest);
    }

    fn with(replace: &str, by: &str) -> Result<Manifest, Invalid> {
        assert!(EXAMPLE.contains(replace), "{replace}");
        Manifest::parse(&EXAMPLE.replacen(replace, by, 1))
    }

    #[test]
    fn unknown_keys_and_tables_are_refused() {
        assert!(with("license = \"MIT\"", "licence = \"MIT\"").is_err());
        assert!(with("network = false", "network = false\nusb = true").is_err());
        assert!(with("input = \"none\"", "input = \"none\"\nmenu = \"x\"").is_err());
        // Contributions this version doesn't build yet.
        for table in [
            "[[filter]]\nid = \"grain\"",
            "[[importer]]\nid = \"c1\"",
            "[[presets]]\npath = \"p/\"",
        ] {
            let text = format!("{EXAMPLE}\n{table}\n");
            assert!(Manifest::parse(&text).is_err(), "{table}");
        }
    }

    #[test]
    fn names_ranges_and_defaults_are_checked() {
        let refused = [
            ("id = \"dev.jane.analog\"", "id = \"analog\""),
            ("id = \"dev.jane.analog\"", "id = \"Dev.Jane.Analog\""),
            ("version = \"1.2.0\"", "version = \"1.2\""),
            ("rawmakase = \">=0.6, <0.7\"", "rawmakase = \"sometime\""),
            ("api = 1", "api = 3"),
            ("api = 1", "api = 0"),
            ("path = \"bin/analog\"", "path = \"../analog\""),
            ("path = \"bin/analog\"", "path = \"/usr/bin/analog\""),
            ("path = \"bin/analog\"", "path = \"bin\\\\analog\""),
            ("path = \"bin/analog\"", "path = \"C:analog\""),
            ("default = \"Tri-X\"", "default = \"Velvia\""),
            ("default = 50", "default = 150"),
            ("default = true", "default = 1"),
            (
                "control = { slider = [0, 100] }",
                "control = { slider = [100, 0] }",
            ),
            ("id = \"strength\"", "id = \"default-stock\""),
            ("id = \"auto-film-look\"", "id = \"Auto\""),
            ("\"develop.auto\"]", "\"develop.sidebar\"]"),
            ("place = \"develop\"", "place = \"toolbar\""),
            ("launch = \"external\"", "launch = \"daemon\""),
            ("start = \"on-demand\"", "start = \"always\""),
            ("files = \"photos\"", "files = \"some\""),
            ("title = \"Auto Film Look\"", "title = \" \""),
        ];
        for (replace, by) in refused {
            assert!(with(replace, by).is_err(), "{by}");
        }
        let without_program = EXAMPLE.replace(
            "[program]\npath = \"bin/analog\"\nstart = \"on-demand\"",
            "",
        );
        assert!(Manifest::parse(&without_program).is_err());
    }

    const GENERATORS: &str = r#"
[extension]
id = "rawmakase.ai-masks"
name = "AI Masks"
version = "1.0.0"
rawmakase = ">=0.6"
api = 2

[program]
path = "bin/ai-masks"

[[mask-generator]]
id = "subject"
input = ["none", "prompt"]
provenance = "subject"
prompt-hint = "Click the subject on the photo, or drag a box around it"
[[mask-generator.tile]]
name = "Subject"
[[mask-generator.tile]]
name = "Background"
invert = true

[[mask-generator]]
id = "sky"
input = ["none"]
provenance = "sky"
[[mask-generator.tile]]
name = "Sky"

[[mask-generator]]
id = "people"
input = ["prompt"]
"#;

    fn generators(replace: &str, by: &str) -> Result<Manifest, Invalid> {
        assert!(GENERATORS.contains(replace), "{replace}");
        Manifest::parse(&GENERATORS.replacen(replace, by, 1))
    }

    #[test]
    fn mask_generators_read_with_their_tiles_and_provenance() {
        let manifest = Manifest::parse(GENERATORS).unwrap();
        let [subject, sky, people] = &manifest.mask_generators[..] else {
            panic!("three generators");
        };
        assert_eq!(subject.input, [Input::None, Input::Prompt]);
        assert_eq!(
            subject.tiles,
            [
                Tile {
                    name: "Subject".into(),
                    invert: false
                },
                Tile {
                    name: "Background".into(),
                    invert: true
                }
            ]
        );
        assert_eq!(subject.provenance(AI_MASKS), "subject");
        assert_eq!(sky.provenance(AI_MASKS), "sky");
        assert_eq!(people.provenance(AI_MASKS), "rawmakase.ai-masks/people");
        assert_eq!(
            manifest.contributions(),
            [
                "mask-generator/subject",
                "mask-generator/sky",
                "mask-generator/people"
            ]
        );
        assert!(manifest.runs_in_app());
        let written = toml::to_string(&manifest).unwrap();
        assert_eq!(Manifest::parse(&written).unwrap(), manifest);
    }

    #[test]
    fn mask_generators_need_api_2_and_a_program() {
        let error = generators("api = 2", "api = 1").unwrap_err();
        assert!(error.0.contains("api = 2"), "{error}");
        let without_program = GENERATORS.replace("[program]\npath = \"bin/ai-masks\"", "");
        assert!(Manifest::parse(&without_program).is_err());
    }

    #[test]
    fn a_mask_generator_claims_only_its_own_provenance() {
        // A third party gets <extension>/<id>, stated or not.
        let third = |provenance: &str| {
            let text = GENERATORS
                .replace("id = \"rawmakase.ai-masks\"", "id = \"dev.jane.masks\"")
                .replacen("provenance = \"subject\"\n", "", 1)
                .replacen("provenance = \"sky\"\n", provenance, 1);
            Manifest::parse(&text)
        };
        third("").unwrap();
        third("provenance = \"dev.jane.masks/sky\"\n").unwrap();
        assert!(third("provenance = \"sky\"\n").is_err());
        assert!(third("provenance = \"subject\"\n").is_err());
        assert!(third("provenance = \"dev.jane.masks/other\"\n").is_err());
        // The bundled extension may claim the saved values, and only those.
        assert!(generators("provenance = \"sky\"", "provenance = \"clouds\"").is_err());
        // One generator per provenance.
        let twice = generators("provenance = \"sky\"", "provenance = \"subject\"").unwrap_err();
        assert!(twice.0.contains("used twice"), "{twice}");
        let stated = generators(
            "input = [\"prompt\"]",
            "input = [\"prompt\"]\nprovenance = \"rawmakase.ai-masks/people\"",
        );
        stated.unwrap();
    }

    #[test]
    fn mask_generator_fields_are_checked() {
        let refused = [
            ("input = [\"none\"]", "input = []"),
            ("input = [\"none\"]", "input = [\"box\"]"),
            ("input = [\"none\"]", "input = [\"brush\"]"),
            ("input = [\"none\"]", "input = [\"point\"]"),
            ("input = [\"none\"]", "input = [\"points\"]"),
            ("input = [\"none\"]", "input = [\"none\", \"none\"]"),
            ("input = [\"none\"]", "input = [\"lasso\"]"),
            ("id = \"people\"", "id = \"sky\""),
            ("id = \"people\"", "id = \"People\""),
            ("name = \"Sky\"", "name = \" \""),
            ("name = \"Sky\"", "name = \"Sky\"\ncolour = \"blue\""),
            ("invert = true", "invert = \"yes\""),
            (
                "prompt-hint = \"Click the subject on the photo, or drag a box around it\"",
                "prompt-hint = \" \"",
            ),
            (
                "input = [\"prompt\"]",
                "input = [\"prompt\"]\nmodel = \"sam\"",
            ),
        ];
        for (replace, by) in refused {
            assert!(generators(replace, by).is_err(), "{by}");
        }
        assert!(generators("input = [\"prompt\"]", "").is_err());
    }

    #[test]
    fn every_place_the_design_names_is_accepted() {
        for place in [
            "library.photo-menu",
            "library.toolbar",
            "library.auto",
            "develop.auto",
            "develop.tool-strip",
            "file.import",
            "file.export",
            "extensions.menu",
        ] {
            let manifest = with("\"develop.auto\"]", &format!("\"{place}\"]")).unwrap();
            assert_eq!(manifest.commands[0].places.len(), 3, "{place}");
        }
    }

    #[test]
    fn more_access_is_listed() {
        let before = Access::default();
        let after = Access {
            network: true,
            files: Files::Any,
            devices: vec!["midi".into()],
        };
        assert_eq!(
            after.more_than(&before),
            ["the network", "any file", "midi devices"]
        );
        assert!(before.more_than(&after).is_empty());
    }

    /// The committed schema is what this crate generates: change the types,
    /// then write it again with `RAWMAKASE_WRITE_SCHEMA=1 cargo test -p
    /// rawmakase-extension-protocol`.
    #[test]
    fn the_committed_schema_is_the_generated_one() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/extensions/manifest.schema.json");
        let generated = serde_json::to_string_pretty(&schema()).unwrap() + "\n";
        if std::env::var_os("RAWMAKASE_WRITE_SCHEMA").is_some() {
            std::fs::write(&path, &generated).unwrap();
        }
        // Compared as JSON, not as text: key order follows serde_json's
        // features, which another crate of the build may change.
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        let committed: Option<Value> = serde_json::from_str(&committed).ok();
        assert!(
            committed.as_ref() == Some(&schema()),
            "{} is out of date: run RAWMAKASE_WRITE_SCHEMA=1 cargo test -p rawmakase-extension-protocol",
            path.display()
        );
    }

    #[test]
    fn the_schema_refuses_unknown_keys() {
        let schema = schema();
        assert_eq!(schema["additionalProperties"], Value::Bool(false));
        assert!(schema["properties"]["control-adapter"].is_object());
        assert!(schema["properties"]["mask-generator"].is_object());
        assert!(schema["properties"].get("filter").is_none());
    }
}
