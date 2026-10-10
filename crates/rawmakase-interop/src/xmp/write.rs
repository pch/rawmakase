//! Writes a recipe back out as Camera Raw settings (`crs:`), the XMP Lightroom
//! embeds in its exports. The keys and scales mirror `apply`, so reading the
//! packet back reproduces the edit.
use crate::xml::{
    self, escape_text,
    ns::{AUX, CRS, DC, LR, PHOTOSHOP, XMP, XMP_MM},
};
use crate::{camera_data::Metadata, color::curve::ToneCurve, model::recipe::Recipe};
use std::fmt::Write;

/// A keyword's path, top first, and which of its names an export writes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeywordPath {
    pub path: Vec<String>,
    pub exported: Vec<bool>,
}
impl KeywordPath {
    /// A path whose names all export.
    #[cfg(any(test, feature = "test-support"))]
    pub fn all(path: Vec<String>) -> Self {
        Self {
            exported: vec![true; path.len()],
            path,
        }
    }
}

/// Facts about the photo that go in the packet beside its settings.
#[derive(Clone, Debug, Default)]
pub struct Photo {
    /// The software writing the packet (xmp:CreatorTool), e.g. "RAWmakase 0.2.0";
    /// left out when empty.
    pub creator_tool: String,
    /// The RAW's file name, e.g. "DSC07924.ARW".
    pub raw_name: String,
    /// Capture time as EXIF writes it, "2018:08:26 10:39:33".
    pub captured: Option<String>,
    /// Capture time as XMP writes it, with subseconds and offset when known;
    /// wins over `captured`.
    pub created: Option<String>,
    /// Export time in UTC, "2026-09-27T06:12:22Z".
    pub now: String,
    pub rating: i32,
    pub label: String,
    pub keywords: Vec<KeywordPath>,
    /// Languages of each, as (language, text), `x-default` first.
    pub title: Vec<(String, String)>,
    pub caption: Vec<(String, String)>,
    pub rights: Vec<(String, String)>,
    /// In order.
    pub creators: Vec<String>,
    /// The lens name (aux:Lens), part of the camera info.
    pub lens: bool,
    /// Include the develop settings, not only the descriptive metadata.
    pub settings: bool,
    /// "image/jpeg" or "image/tiff".
    pub format: String,
    /// The crop as rendered, `[left, top, right, bottom]`: with Constrain Crop, the
    /// crop constrained to the photo (`develop::rendered_crop`). `None` writes the
    /// recipe's own crop.
    pub crop: Option<[f32; 4]>,
}

/// "2018:08:26 10:39:33" as XMP's "2018-08-26T10:39:33".
fn xmp_date(exif: &str) -> Option<String> {
    let (date, time) = exif.trim().split_once(' ')?;
    (date.len() == 10).then(|| format!("{}T{time}", date.replace(':', "-")))
}

/// Lightroom's number style: integers for ×100 sliders, a sign on signed ones.
fn number(value: f32, decimals: usize, signed: bool) -> String {
    let text = format!("{value:.decimals$}");
    let zero = text.trim_start_matches(['-', '0', '.']).is_empty();
    if zero {
        format!("{:.decimals$}", 0.)
    } else if signed && value > 0. {
        format!("+{text}")
    } else {
        text
    }
}

pub(super) struct Settings(pub(super) Vec<(String, String)>);
impl Settings {
    /// `value` in recipe units, written as `value / scale`.
    fn put(&mut self, key: &str, value: f32, scale: f32, decimals: usize, signed: bool) {
        self.0
            .push((key.into(), number(value / scale, decimals, signed)));
    }
    fn text(&mut self, key: &str, value: impl Into<String>) {
        self.0.push((key.into(), value.into()));
    }
}

/// An enhanced profile (Adobe Color, a creative look) as Lightroom writes it: a `Look`
/// element over its base profile, which `CameraProfile` names. Indented for a
/// settings description.
pub(super) fn look_element(r: &Recipe) -> Option<String> {
    let profile = r.profile.as_ref()?;
    let look = profile.enhanced.as_ref()?;
    let amount = if look.amount.is_some() {
        r.profile_amount
    } else {
        1.
    };
    Some(format!(
        "   <crs:Look>\n    <rdf:Description\n     crs:Name=\"{}\"\n     crs:Amount=\"{}\"\n     crs:UUID=\"{}\"\n     crs:SupportsAmount=\"{}\"/>\n   </crs:Look>\n",
        escape_text(&profile.name),
        (amount * 100.).round() / 100.,
        escape_text(&look.uuid),
        look.amount.is_some(),
    ))
}

/// Lightroom's lens profile Setup and the profile the edit uses: the one rendering
/// when the photo is known, else the one the edit names.
fn lens_profile(s: &mut Settings, r: &Recipe, m: Option<&Metadata>) {
    let choice = &r.lens_profile_choice;
    s.text("LensProfileSetup", choice.setup.xmp());
    let resolved = m.map(|m| r.lens_profile_in_use(m));
    let in_use = resolved.as_ref().and_then(|r| r.used);
    let missing = resolved.as_ref().is_some_and(|r| r.missing.is_some());
    let id = match (in_use, &choice.id) {
        // A profile the edit names that isn't imported stays named, so the edit
        // finds it again once it is; the digest the edit recorded still describes
        // the same file.
        (Some(c), Some(id))
            if missing
                || (c.profile.is(&id.filename, &id.name)
                    && (id.name.is_empty() || id.name == c.profile.name)) =>
        {
            id.clone()
        }
        (Some(c), _) => crate::lens::choice::LensProfileId::of(&c.profile),
        (None, Some(id)) => id.clone(),
        (None, None) => return,
    };
    if !id.name.is_empty() {
        s.text("LensProfileName", id.name);
    }
    if !id.filename.is_empty() {
        s.text("LensProfileFilename", id.filename);
    }
    if !id.digest.is_empty() {
        s.text("LensProfileDigest", id.digest);
    }
    if id.embedded {
        s.text("LensProfileIsEmbedded", "True");
    }
}

/// The photo a packet's settings are written for.
#[derive(Clone, Copy)]
pub(super) struct Frame<'a> {
    pub(super) metadata: &'a Metadata,
    /// The crop as rendered (see [`Photo::crop`]).
    pub(super) crop: Option<[f32; 4]>,
}

pub(super) fn settings(r: &Recipe, photo: Option<Frame<'_>>) -> Settings {
    let m = photo.map(|p| p.metadata);
    let mut s = Settings(Vec::new());
    s.text("ProcessVersion", "11.0");
    if let Some(profile) = &r.profile {
        let name = match &profile.enhanced {
            Some(look) => &look.base_name,
            None => &profile.name,
        };
        s.text("CameraProfile", name.clone());
    }
    s.text("WhiteBalance", "Custom");
    s.put("Temperature", r.temperature, 1., 0, false);
    s.put("Tint", r.tint, 1., 0, true);
    s.put("Exposure2012", r.exposure, 1., 2, true);
    for (key, value) in [
        ("Contrast2012", r.contrast),
        ("Highlights2012", r.highlights),
        ("Shadows2012", r.shadows),
        ("Whites2012", r.whites),
        ("Blacks2012", r.blacks),
        ("Texture", r.effects.texture),
        ("Clarity2012", r.effects.clarity),
        ("Dehaze", r.effects.dehaze),
        ("Vibrance", r.vibrance),
        ("Saturation", r.saturation),
    ] {
        s.put(key, value, 0.01, 0, true);
    }
    for (i, name) in ["Shadows", "Darks", "Lights", "Highlights"]
        .iter()
        .enumerate()
    {
        s.put(
            &format!("Parametric{name}"),
            r.effects.parametric[i],
            0.01,
            0,
            true,
        );
    }
    s.put("CurveRefineSaturation", r.curve_saturation, 0.01, 0, false);
    for (i, name) in ["Shadow", "Midtone", "Highlight"].iter().enumerate() {
        s.put(
            &format!("Parametric{name}Split"),
            r.effects.splits[i],
            0.01,
            0,
            false,
        );
    }
    s.put("Sharpness", r.sharpening, 1. / 150., 0, false);
    s.put("SharpenRadius", r.sharpening_radius, 1., 1, true);
    s.put("SharpenDetail", r.sharpening_detail, 0.01, 0, false);
    s.put("SharpenEdgeMasking", r.sharpening_masking, 0.01, 0, false);
    s.put("LuminanceSmoothing", r.noise_luma, 0.01, 0, false);
    s.put(
        "LuminanceNoiseReductionDetail",
        r.effects.luma_detail,
        0.01,
        0,
        false,
    );
    s.put(
        "LuminanceNoiseReductionContrast",
        r.effects.luma_contrast,
        0.01,
        0,
        false,
    );
    s.put("ColorNoiseReduction", r.noise_chroma, 0.01, 0, false);
    s.put(
        "ColorNoiseReductionDetail",
        r.effects.chroma_detail,
        0.01,
        0,
        false,
    );
    s.put(
        "ColorNoiseReductionSmoothness",
        r.effects.chroma_smoothness,
        0.01,
        0,
        false,
    );
    let bands = [
        "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
    ];
    for (j, control) in ["Hue", "Saturation", "Luminance"].iter().enumerate() {
        for (i, band) in bands.iter().enumerate() {
            s.put(
                &format!("{control}Adjustment{band}"),
                r.hsl[i][j],
                0.01,
                0,
                true,
            );
        }
    }
    // Black & white by Treatment or by a black & white profile, as Lightroom writes it.
    let black_white = r.treatment() == crate::model::recipe::Treatment::BlackWhite;
    s.text(
        "ConvertToGrayscale",
        if black_white { "True" } else { "False" },
    );
    if black_white {
        for (i, band) in bands.iter().enumerate() {
            s.put(
                &format!("GrayMixer{band}"),
                r.effects.gray_mix[i],
                0.01,
                0,
                true,
            );
        }
    }
    for (i, name) in [(0, "Shadow"), (2, "Highlight")] {
        s.put(
            &format!("SplitToning{name}Hue"),
            r.grading[i][0],
            1. / 360.,
            0,
            false,
        );
        s.put(
            &format!("SplitToning{name}Saturation"),
            r.grading[i][1],
            0.01,
            0,
            false,
        );
        s.put(
            &format!("ColorGrade{name}Lum"),
            r.grading[i][2],
            0.01,
            0,
            true,
        );
    }
    s.put("SplitToningBalance", r.effects.balance, 0.01, 0, true);
    s.put("ColorGradeMidtoneHue", r.grading[1][0], 1. / 360., 0, false);
    s.put("ColorGradeMidtoneSat", r.grading[1][1], 0.01, 0, false);
    s.put("ColorGradeMidtoneLum", r.grading[1][2], 0.01, 0, true);
    s.put("ColorGradeBlending", r.effects.blending, 0.01, 0, false);
    s.put(
        "ColorGradeGlobalHue",
        r.effects.global_grade[0],
        1. / 360.,
        0,
        false,
    );
    s.put(
        "ColorGradeGlobalSat",
        r.effects.global_grade[1],
        0.01,
        0,
        false,
    );
    s.put(
        "ColorGradeGlobalLum",
        r.effects.global_grade[2],
        0.01,
        0,
        true,
    );
    for (i, band) in ["Red", "Green", "Blue"].iter().enumerate() {
        s.put(
            &format!("{band}Hue"),
            r.effects.calibration[i][0],
            0.01,
            0,
            true,
        );
        s.put(
            &format!("{band}Saturation"),
            r.effects.calibration[i][1],
            0.01,
            0,
            true,
        );
    }
    s.put("ShadowTint", r.effects.shadow_tint, 0.01, 0, true);
    s.put("GrainAmount", r.effects.grain, 0.01, 0, false);
    if r.effects.grain > 0. {
        s.put("GrainSize", r.effects.grain_size, 0.01, 0, false);
        s.put("GrainFrequency", r.effects.grain_roughness, 0.01, 0, false);
        s.text("GrainSeed", r.effects.grain_seed.to_string());
    }
    s.put("PostCropVignetteAmount", r.effects.vignette, 0.01, 0, true);
    if r.effects.vignette != 0. {
        s.put(
            "PostCropVignetteMidpoint",
            r.effects.vignette_midpoint,
            0.01,
            0,
            false,
        );
        s.put(
            "PostCropVignetteRoundness",
            r.effects.vignette_roundness,
            0.01,
            0,
            true,
        );
        s.put(
            "PostCropVignetteFeather",
            r.effects.vignette_feather,
            0.01,
            0,
            false,
        );
        s.put(
            "PostCropVignetteHighlightContrast",
            r.effects.vignette_highlights,
            0.01,
            0,
            false,
        );
        s.text(
            "PostCropVignetteStyle",
            r.effects.vignette_style.code().to_string(),
        );
    }
    s.put("VignetteAmount", r.effects.lens_vignette, 0.01, 0, true);
    s.put(
        "VignetteMidpoint",
        r.effects.lens_vignette_midpoint,
        0.01,
        0,
        false,
    );
    for (i, name) in ["Purple", "Green"].iter().enumerate() {
        s.put(
            &format!("Defringe{name}Amount"),
            r.effects.defringe[i],
            0.05,
            0,
            false,
        );
        s.put(
            &format!("Defringe{name}HueLo"),
            r.effects.defringe_ranges[i][0],
            0.01,
            0,
            false,
        );
        s.put(
            &format!("Defringe{name}HueHi"),
            r.effects.defringe_ranges[i][1],
            0.01,
            0,
            false,
        );
    }
    s.text("LensProfileEnable", if r.lens_profile { "1" } else { "0" });
    lens_profile(&mut s, r, m);
    s.text("AutoLateralCA", if r.lens_ca { "1" } else { "0" });
    s.put(
        "LensProfileDistortionScale",
        r.lens_distortion,
        0.01,
        0,
        false,
    );
    s.put(
        "LensProfileVignettingScale",
        r.lens_vignetting,
        0.01,
        0,
        false,
    );
    s.put(
        "LensManualDistortionAmount",
        r.lens_manual_distortion,
        0.01,
        0,
        true,
    );
    // Lightroom's panel switches, written only for panels switched off.
    for panel in r.panels.switched_off() {
        for key in panel.lightroom_keys() {
            s.text(key, "False");
        }
    }
    let t = &r.transform;
    s.put("PerspectiveVertical", t.vertical, 0.01, 0, true);
    s.put("PerspectiveHorizontal", t.horizontal, 0.01, 0, true);
    s.put("PerspectiveRotate", t.rotate, 1., 1, true);
    s.put("PerspectiveAspect", t.aspect, 0.01, 0, true);
    s.put("PerspectiveScale", t.scale, 0.01, 0, false);
    s.put("PerspectiveX", t.offset_x, 0.01, 2, true);
    s.put("PerspectiveY", t.offset_y, 0.01, 2, true);
    let u = &r.upright;
    s.text("PerspectiveUpright", u.mode.code().to_string());
    if !u.corrections.is_empty() {
        s.text("UprightTransformCount", u.corrections.len().to_string());
    }
    for (i, m) in u.corrections.iter().enumerate() {
        let m: Vec<String> = m.iter().map(|x| format!("{x:.9}")).collect();
        s.text(&format!("UprightTransform_{i}"), m.join(","));
    }
    // Guided's guides as Camera Raw writes them: "x1,y1,x2,y2" with nine decimals.
    if !u.guides.is_empty() {
        s.text("UprightFourSegmentsCount", u.guides.len().to_string());
    }
    for (i, g) in u.guides.iter().enumerate() {
        // The shortest decimal that reads back as the same number, so 0.7 is written
        // 0.700000000 rather than its nearest single-precision value.
        let fixed = |v: f32| format!("{:.9}", v.to_string().parse::<f64>().unwrap_or(0.));
        let ends = [g.a[0], g.a[1], g.b[0], g.b[1]].map(fixed);
        s.text(&format!("UprightFourSegments_{i}"), ends.join(","));
    }
    for (key, value) in &u.lightroom {
        s.text(key, value.clone());
    }
    // With Constrain Crop, the crop as rendered: Camera Raw renders the stored crop as it
    // is, and Lightroom stores the crop it constrained. Without the photo (a preset),
    // the crop as drawn, which the photo it is applied to constrains again.
    let crop = photo.and_then(|p| p.crop).unwrap_or(r.crop);
    for (i, name) in ["Left", "Top", "Right", "Bottom"].iter().enumerate() {
        s.put(&format!("Crop{name}"), crop[i], 1., 6, false);
    }
    s.put("CropAngle", r.straighten, 1., 2, true);
    s.text(
        "CropConstrainToWarp",
        if r.constrain_crop { "1" } else { "0" },
    );
    let cropped = crop != [0., 0., 1., 1.] || r.straighten != 0.;
    s.text("HasCrop", if cropped { "True" } else { "False" });
    s.text("HasSettings", "True");
    s
}

pub(super) fn curve(out: &mut String, name: &str, c: &ToneCurve) {
    let _ = write!(out, "   <crs:{name}>\n    <rdf:Seq>\n");
    for [x, y] in &c.points {
        let _ = writeln!(
            out,
            "     <rdf:li>{}, {}</rdf:li>",
            (x * 255.).round() as i32,
            (y * 255.).round() as i32
        );
    }
    let _ = write!(out, "    </rdf:Seq>\n   </crs:{name}>\n");
}

/// What to write for a recipe without Point Color swatches.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NoPointColors {
    /// Nothing: a photo's settings without swatches.
    Omit,
    /// Lightroom's empty selection, so that applying a preset clears swatches.
    EmptySelection,
}

/// `crs:PointColors` and `crs:ColorVariance`, one item per swatch.
pub(super) fn point_colors(out: &mut String, r: &Recipe, empty: NoPointColors) {
    use crate::model::point_color::{ListText, format_list};
    let text = if r.point_colors.is_empty() {
        if empty == NoPointColors::Omit {
            return;
        }
        ListText {
            points: vec![["-1.000000"; 19].join(", ")],
            variances: vec!["-50.000000".into()],
        }
    } else {
        format_list(&r.point_colors)
    };
    for (name, items) in [
        ("PointColors", text.points),
        ("ColorVariance", text.variances),
    ] {
        let _ = write!(out, "   <crs:{name}>\n    <rdf:Seq>\n");
        for item in items {
            let _ = writeln!(out, "     <rdf:li>{item}</rdf:li>");
        }
        let _ = write!(out, "    </rdf:Seq>\n   </crs:{name}>\n");
    }
}

/// A language alternative (dc:title, dc:description, dc:rights); nothing
/// when it has no text.
fn lang_alt(out: &mut String, name: &str, langs: &[(String, String)]) {
    if langs.is_empty() {
        return;
    }
    let _ = write!(out, "   <{name}>\n    <rdf:Alt>\n");
    for (lang, text) in langs {
        let _ = writeln!(
            out,
            "     <rdf:li xml:lang=\"{}\">{}</rdf:li>",
            escape_text(lang),
            escape_text(text)
        );
    }
    let _ = write!(out, "    </rdf:Alt>\n   </{name}>\n");
}
/// An unordered (Bag) or ordered (Seq) list; nothing when empty.
fn list(out: &mut String, name: &str, kind: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    let _ = write!(out, "   <{name}>\n    <rdf:{kind}>\n");
    for item in items {
        let _ = writeln!(out, "     <rdf:li>{}</rdf:li>", escape_text(item));
    }
    let _ = write!(out, "    </rdf:{kind}>\n   </{name}>\n");
}
/// dc:subject, every keyword and its ancestors once, and
/// lr:hierarchicalSubject, each keyword's path joined with "|" (a top-level
/// keyword as a one-name path, as Lightroom writes it). Names Lightroom's
/// keyword options leave out of exports are left out of both: a keyword
/// whose parents do not all export goes in the hierarchy alone. "|" always
/// separates there, so a path with a name containing it is left out of it.
pub fn keyword_lists(keywords: &[KeywordPath]) -> (Vec<String>, Vec<String>) {
    let mut subject: Vec<String> = Vec::new();
    let mut hierarchical: Vec<String> = Vec::new();
    for keyword in keywords {
        let names: Vec<(&str, bool)> = keyword
            .path
            .iter()
            .zip(keyword.exported.iter().chain(std::iter::repeat(&true)))
            .map(|(n, e)| (n.trim(), *e))
            .filter(|(n, _)| !n.is_empty())
            .collect();
        if !names.last().is_some_and(|(_, e)| *e) {
            continue;
        }
        for (name, exported) in &names {
            if *exported && !subject.iter().any(|s| s == name) {
                subject.push(name.to_string());
            }
        }
        let path: Vec<&str> = if names.iter().all(|(_, e)| *e) {
            names.iter().map(|(n, _)| *n).collect()
        } else {
            names.last().map(|(n, _)| *n).into_iter().collect()
        };
        if !path.is_empty() && path.iter().all(|n| !n.contains('|')) {
            let joined = path.join("|");
            if !hierarchical.contains(&joined) {
                hierarchical.push(joined);
            }
        }
    }
    (subject, hierarchical)
}

/// The XMP packet for an exported photo.
pub fn packet(r: &Recipe, m: &Metadata, photo: &Photo) -> String {
    let mut attributes: Vec<(String, String)> = Vec::new();
    if !photo.creator_tool.is_empty() {
        attributes.push(("xmp:CreatorTool".into(), photo.creator_tool.clone()));
    }
    attributes.push(("xmp:ModifyDate".into(), photo.now.clone()));
    attributes.push(("xmp:MetadataDate".into(), photo.now.clone()));
    let created = photo
        .created
        .clone()
        .or_else(|| photo.captured.as_deref().and_then(xmp_date));
    if let Some(date) = created {
        attributes.push(("xmp:CreateDate".into(), date.clone()));
        attributes.push(("photoshop:DateCreated".into(), date));
    }
    if photo.rating != 0 {
        attributes.push(("xmp:Rating".into(), photo.rating.to_string()));
    }
    if !photo.label.is_empty() {
        attributes.push(("xmp:Label".into(), photo.label.clone()));
    }
    if photo.lens && !m.lens_model.is_empty() {
        attributes.push(("aux:Lens".into(), m.lens_model.clone()));
    }
    if !photo.raw_name.is_empty() {
        attributes.push(("xmpMM:PreservedFileName".into(), photo.raw_name.clone()));
    }
    attributes.push(("dc:format".into(), photo.format.clone()));
    if photo.settings {
        if !photo.raw_name.is_empty() {
            attributes.push(("crs:RawFileName".into(), photo.raw_name.clone()));
        }
        attributes.extend(
            settings(
                r,
                Some(Frame {
                    metadata: m,
                    crop: photo.crop,
                }),
            )
            .0
            .into_iter()
            .map(|(k, v)| (format!("crs:{k}"), v)),
        );
        attributes.push(("crs:AlreadyApplied".into(), "True".into()));
    }
    let mut out = format!(
        "  <rdf:Description rdf:about=\"\"\n    \
         xmlns:xmp=\"{XMP}\"\n    \
         xmlns:aux=\"{AUX}\"\n    \
         xmlns:photoshop=\"{PHOTOSHOP}\"\n    \
         xmlns:xmpMM=\"{XMP_MM}\"\n    \
         xmlns:dc=\"{DC}\"\n    \
         xmlns:lr=\"{LR}\"\n    \
         xmlns:crs=\"{CRS}\""
    );
    for (key, value) in &attributes {
        let _ = write!(out, "\n   {key}=\"{}\"", escape_text(value));
    }
    out.push_str(">\n");
    lang_alt(&mut out, "dc:title", &photo.title);
    lang_alt(&mut out, "dc:description", &photo.caption);
    lang_alt(&mut out, "dc:rights", &photo.rights);
    list(&mut out, "dc:creator", "Seq", &photo.creators);
    let (subject, hierarchical) = keyword_lists(&photo.keywords);
    list(&mut out, "dc:subject", "Bag", &subject);
    list(&mut out, "lr:hierarchicalSubject", "Bag", &hierarchical);
    if photo.settings {
        out.extend(look_element(r));
        curve(&mut out, "ToneCurvePV2012", &r.curve);
        for (i, name) in ["Red", "Green", "Blue"].iter().enumerate() {
            curve(
                &mut out,
                &format!("ToneCurvePV2012{name}"),
                &r.effects.channels[i],
            );
        }
        point_colors(&mut out, r, NoPointColors::Omit);
    }
    out.push_str("  </rdf:Description>\n");
    xml::packet(&out)
}
