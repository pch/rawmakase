//! The per-pixel stage's inputs, flattened for the GPU port in `gpu/develop.wgsl`.
//! Only the current engine's reference path is ported: engine 4 with reference curves,
//! color and calibration and a profile tone curve, which every new photo uses. Other
//! recipes return `None` and render on the CPU, which stays the reference.
use super::{CurveSet, Source, profile_matrix};
use crate::develop::local_tone::LocalToneMap;
use crate::model::recipe::Recipe;

/// Named slots of the parameter array and their lengths. `wgsl_prelude` turns them into
/// `P_*` index constants for the shader.
const FIELDS: &[(&str, usize)] = &[
    ("CAMERA", 9),
    ("WB", 3),
    ("HUE", 5),
    ("HUE2", 1),
    ("HUE_WEIGHT", 1),
    ("PROFILE_SCALE", 1),
    ("CALIBRATION", 9),
    ("SHADOW_TINT", 1),
    ("EXPOSURE", 1),
    ("RAMP", 4),
    ("LOOK", 5),
    ("ENH", 5),
    ("ENH_CURVE", 1),
    ("TONE", 1),
    ("TONE_COUNT", 1),
    // 1 when the tone curve's knots are at i / (count - 1), as Adobe's are: the shader
    // then indexes the curve instead of searching it.
    ("TONE_UNIFORM", 1),
    ("LOCAL", 1),
    ("LOCAL_SIZE", 2),
    ("LOCAL_SCALE", 2),
    ("LOCAL_A", 3),
    ("SHADOWS", 4),
    ("HIGHLIGHTS", 4),
    ("BASIC", 1),
    ("LEVELS", 3),
    ("PARAMETRIC_ON", 1),
    ("PARAMETRIC", 4),
    ("SPLITS", 3),
    // The measured parametric curve's table, or -1 (see `parametric::ParametricCurve`).
    ("PARAMETRIC_LUT", 1),
    ("MASTER", 1),
    ("REFINE_SATURATION", 1),
    ("CHANNELS", 3),
    ("MIXER", 1),
    // `black_white::gray_grid`, or -1.
    ("GRAY_GRID", 1),
    // `ColorMixer::saturation_gray`: how far colors fade to their luminance.
    ("SATURATION_GRAY", 1),
    // `ColorMixer::gray_source`: its grid, or -1.
    ("GRAY_SOURCE", 1),
    // Point Color: table offset and number of swatches (see `point_color::params`).
    ("POINT", 2),
    // A look's RGB table: samples offset, dimensions, divisions, gamma, gamut and
    // amount, then the matrices into its primaries and back.
    ("RGB", 6),
    ("RGB_INTO", 9),
    ("RGB_BACK", 9),
    // Color grading: tables, samples and operator (see `color_grade::ColorGrade`).
    ("GRADE", 4),
    ("ADJUST", 1),
    ("DEFRINGE", 2),
    ("DEFRINGE_RANGES", 4),
    // 1 when out-of-gamut colors clip per channel (`GamutModel::Clip`).
    ("GAMUT_CLIP", 1),
    ("MONO", 1),
    ("GRAY_MIX", 8),
    ("TONE_ONLY", 1),
    // Masks (see `masks::local`): how many, weight words per pixel, their deltas.
    ("MASKS", 1),
    ("MASK_WORDS", 1),
    ("MASK_DELTAS", 1),
    ("EXPOSURE_EV", 1),
    ("LOCAL_WB", 6),
    ("LOCAL_TONE", 1),
    // The masks' Contrast pivot, or -1 for the original Contrast before Whites and Blacks.
    ("LOCAL_PIVOT", 1),
    ("LOCAL_FAMILIES", 1),
    ("LOCAL_KEYS", 2),
    ("GLOBAL_SH", 2),
];
pub(crate) fn wgsl_prelude() -> String {
    use crate::develop::point_color::{CONSTANT_PARAMS, SWATCH_PARAMS};
    let mut at = 0;
    let fields: String = FIELDS
        .iter()
        .map(|(name, len)| {
            let line = format!("const P_{name}: u32 = {at}u;\n");
            at += len;
            line
        })
        .collect();
    fields
        + &format!(
            "const POINT_CONSTANTS: i32 = {CONSTANT_PARAMS};\nconst POINT_SWATCH: i32 = {SWATCH_PARAMS};\n"
        )
}
/// Where parameter `name` starts in `PixelParams::params`, and its length.
fn field(name: &str) -> (usize, usize) {
    let mut at = 0;
    for (field, len) in FIELDS {
        if *field == name {
            return (at, *len);
        }
        at += len;
    }
    unreachable!("Unknown parameter {name}");
}
pub(crate) struct PixelParams {
    pub(crate) params: Vec<f32>,
    pub(crate) tables: Vec<f32>,
    /// Mask weights, four bytes per word, `MASK_WORDS` words per pixel; empty
    /// without masks.
    pub(crate) weights: Vec<u32>,
    /// The recipe as the tone stage reads it, when the develop pass may keep the tone
    /// stage's output for its samples and reuse it while only the stages after it
    /// change (see `gpu::develop::Kept`). `None` runs the tone stage every time.
    pub(crate) tone: Option<Recipe>,
    /// The Shadows/Highlights map's base on the device, when it was built there: its
    /// coefficients follow `tables` and its keys go to `LOCAL_KEYS` as the pass is
    /// recorded (see [`Self::uploaded`]).
    pub(crate) map: Option<crate::develop::gpu::DeviceMap>,
}
impl PixelParams {
    /// Parameters of zeros but for `values`, without tables.
    #[cfg(test)]
    pub(crate) fn with(values: &[(&str, &[f32])]) -> Self {
        let mut p = Self {
            params: vec![0.; FIELDS.iter().map(|f| f.1).sum()],
            tables: Vec::new(),
            weights: Vec::new(),
            tone: None,
            map: None,
        };
        for (name, v) in values {
            p.set(name, v);
        }
        p
    }
    fn set(&mut self, name: &str, values: &[f32]) {
        let (at, len) = field(name);
        assert_eq!(values.len(), len, "{name}");
        self.params[at..at + len].copy_from_slice(values);
    }
    /// The parameters as the develop pass reads them: with a map on the device, its
    /// coefficients a and b are placed after the tables.
    pub(crate) fn uploaded(&self) -> std::borrow::Cow<'_, [f32]> {
        let Some(map) = &self.map else {
            return self.params.as_slice().into();
        };
        let mut params = self.params.clone();
        let at = field("LOCAL_A").0;
        let a = self.tables.len();
        params[at..at + 2].copy_from_slice(&[a as f32, (a + map.cells()) as f32]);
        params.into()
    }
    /// Where in the parameters, in bytes, a map on the device puts its keys.
    pub(crate) fn keys_byte_offset() -> u64 {
        field("LOCAL_KEYS").0 as u64 * 4
    }
    /// The values of parameter `name`.
    pub(crate) fn get(&self, name: &str) -> &[f32] {
        let (at, len) = field(name);
        &self.params[at..at + len]
    }
    /// Appends a table and returns its offset as a parameter value.
    fn push(&mut self, values: impl IntoIterator<Item = f32>) -> f32 {
        let at = self.tables.len();
        self.tables.extend(values);
        at as f32
    }
    fn table(&mut self, name: &str, table: Option<&crate::camera_profiles::Table>) {
        let Some(t) = table else {
            return self.set(name, &[-1., 0., 0., 0., 0.]);
        };
        let at = self.push(t.data().iter().flatten().copied());
        let [a, b, c] = t.dims();
        self.set(
            name,
            &[at, a as f32, b as f32, c as f32, t.srgb() as u8 as f32],
        );
    }
}
/// Whether a piecewise-linear curve's knots sit at i / (n - 1), so the knot at or
/// below `x` is the one at `floor(x * (n - 1))`.
fn uniform_knots(curve: &[[f32; 2]]) -> bool {
    let n = curve.len();
    n >= 2
        && curve
            .iter()
            .enumerate()
            .all(|(i, p)| p[0] == i as f32 / (n - 1) as f32)
}
/// Whether the GPU port renders this (resolved) recipe.
pub(crate) fn supported(r: &Recipe) -> bool {
    let grading = r.grading.iter().any(|g| g[1] != 0. || g[2] != 0.)
        || r.effects.global_grade[1] != 0.
        || r.effects.global_grade[2] != 0.;
    r.engine >= 4
        && r.reference_curves
        && r.reference_color
        && r.reference_calibration
        && r.profile_tone
        && r.profile.is_some()
        // The original operator's Blending and Balance outside its tables use the
        // older operator.
        && (!grading || crate::develop::color_grade::ColorGrade::new(r).is_some())
}
/// Parameters for `im`'s per-pixel stage with the resolved recipe `r`, or `None` when
/// the GPU port does not cover it.
pub(crate) fn pixel_params(im: Source, r: &Recipe) -> Option<PixelParams> {
    if !supported(r) {
        return None;
    }
    let matrix = profile_matrix(&im.metadata, r);
    fill(
        r,
        CurveSet::for_image(im, r, matrix, masks_need_map(r)),
        matrix,
    )
}
/// Whether active masks change Shadows or Highlights, which read the map.
fn masks_need_map(r: &Recipe) -> bool {
    r.masks
        .iter()
        .any(|m| m.is_active() && (m.adjust.shadows != 0. || m.adjust.highlights != 0.))
}
/// Whether `r`'s per-pixel stage needs the Shadows/Highlights map of the photo, which
/// also carries the measured Clarity.
pub(crate) fn needs_map(r: &Recipe) -> bool {
    r.engine >= 4
        && r.reference_curves
        && (r.shadows != 0.
            || r.highlights != 0.
            || crate::develop::clarity::measured(r) != 0.
            || masks_need_map(r))
}
/// Whether a render needs the photo reduced for the Shadows/Highlights map or for
/// measuring the photo's Contrast pivot; the stage cache keeps it between renders.
pub(crate) fn needs_reduced(r: &Recipe) -> bool {
    needs_map(r) || super::measures_contrast_pivot(r) || super::measures_whites(r)
}
/// Parameters that stop after the tone stage (`tone_stage`, before the map), to tone
/// the reduced photo the Shadows/Highlights map is built from on the GPU.
#[cfg(test)]
pub(crate) fn tone_params(im: Source, r: &Recipe) -> Option<PixelParams> {
    if !supported(r) {
        return None;
    }
    let matrix = profile_matrix(&im.metadata, r);
    let mut p = fill(r, CurveSet::with_photo_measures(im, r, matrix), matrix)?;
    p.set("TONE_ONLY", &[1.]);
    Some(p)
}
/// Parameters with the photo's measures already taken, for a recipe whose per-pixel
/// stage needs no Shadows/Highlights map (see [`needs_map`]); `tone_only` stops them
/// after the tone stage, to tone the reduced photo the map is built from. The same
/// parameters then run the final pass (`with_map`), so they carry the photo's Contrast
/// pivot.
pub(crate) fn measured_params(
    im: Source,
    r: &Recipe,
    measures: super::PhotoMeasures,
    tone_only: bool,
) -> Option<PixelParams> {
    if !supported(r) {
        return None;
    }
    let matrix = profile_matrix(&im.metadata, r);
    let mut p = fill(r, CurveSet::with_measures(r, measures), matrix)?;
    if tone_only {
        p.set("TONE_ONLY", &[1.]);
    }
    Some(p)
}
fn set_local(p: &mut PixelParams, local: &LocalToneMap) {
    p.set("LOCAL", &[1.]);
    p.set("LOCAL_KEYS", &local.keys);
    p.set("LOCAL_SIZE", &[local.width as f32, local.height as f32]);
    p.set("LOCAL_SCALE", &local.scale);
    let a = p.push(local.a.iter().copied());
    let b = p.push(local.b.iter().copied());
    let clarity = match &local.clarity {
        Some(c) => p.push(c.iter().copied()),
        None => -1.,
    };
    p.set("LOCAL_A", &[a, b, clarity]);
    set_curves(p, [&local.shadows, &local.highlights]);
}
/// `tone` parameters for the whole stage, with the map's base built on the device
/// from their result, for a `source`-sized photo. Its coefficients and keys are
/// placed as the pass is recorded; the Clarity measured on the map is not covered.
pub(crate) fn with_device_map(
    mut p: PixelParams,
    map: crate::develop::gpu::DeviceMap,
    source: [u32; 2],
    sliders: crate::develop::local_tone::Sliders,
) -> PixelParams {
    p.set("TONE_ONLY", &[0.]);
    p.set("LOCAL", &[1.]);
    p.set("LOCAL_SIZE", &map.size.map(|v| v as f32));
    p.set(
        "LOCAL_SCALE",
        &crate::develop::local_tone::scale(map.size, source),
    );
    p.set("LOCAL_A", &[-1., -1., -1.]);
    let [shadows, highlights] = crate::develop::local_tone::curves(sliders);
    set_curves(&mut p, [&shadows, &highlights]);
    p.map = Some(map);
    p
}
/// The Shadows and Highlights curves; the shader takes their keys from `LOCAL_KEYS`.
fn set_curves(p: &mut PixelParams, curves: [&Option<crate::develop::local_tone::Curve>; 2]) {
    for (name, curve) in ["SHADOWS", "HIGHLIGHTS"].into_iter().zip(curves) {
        let values = match curve {
            Some(c) => [p.push(c.table), c.key, c.lo, c.hi],
            None => [-1., 0., 0., 0.],
        };
        p.set(name, &values);
    }
}
fn set_rgb_table(p: &mut PixelParams, look: Option<&crate::camera_profiles::RgbLook>) {
    use crate::camera_profiles::{Dimensions, Gamut};
    let Some(look) = look else {
        return p.set("RGB", &[-1., 0., 0., 0., 0., 0.]);
    };
    let t = look.table();
    let at = p.push(t.samples().iter().flatten().copied());
    let dimensions = match t.dimensions() {
        Dimensions::One => 1.,
        Dimensions::Three => 3.,
    };
    let extend = match t.gamut() {
        Gamut::Clip => 0.,
        Gamut::Extend => 1.,
    };
    p.set(
        "RGB",
        &[
            at,
            dimensions,
            t.divisions() as f32,
            t.gamma().code() as f32,
            extend,
            look.amount(),
        ],
    );
    let [into, back] = t.matrices();
    p.set("RGB_INTO", into.as_flattened());
    p.set("RGB_BACK", back.as_flattened());
}
/// `tone` parameters for the whole stage, with the map built from their result.
pub(crate) fn with_map(mut p: PixelParams, map: &LocalToneMap) -> PixelParams {
    p.set("TONE_ONLY", &[0.]);
    set_local(&mut p, map);
    p
}
fn fill(r: &Recipe, lut: CurveSet, matrix: [[f32; 3]; 3]) -> Option<PixelParams> {
    let profile = r.profile.as_ref()?;
    let len = FIELDS.iter().map(|f| f.1).sum();
    let mut p = PixelParams {
        params: vec![0.; len],
        tables: Vec::new(),
        tone: None,
        weights: Vec::new(),
        map: None,
    };
    p.set("CAMERA", matrix.as_flattened());
    p.set("WB", &r.wb);
    let t = profile.gpu_tables(r.temperature);
    p.table("HUE", t.hue.map(|h| h.0));
    let hue2 = match t.hue.and_then(|h| h.1) {
        Some(table) => p.push(table.data().iter().flatten().copied()),
        None => -1.,
    };
    p.set("HUE2", &[hue2]);
    p.set("HUE_WEIGHT", &[t.hue.map_or(0., |h| h.2)]);
    p.set("PROFILE_SCALE", &[t.exposure_scale]);
    p.set("CALIBRATION", lut.calibration.matrix.as_flattened());
    p.set("SHADOW_TINT", &[lut.calibration.shadow]);
    p.set("EXPOSURE", &[lut.exposure_gain]);
    let ramp = lut.black_ramp.as_ref()?;
    p.set("RAMP", &[ramp.black, ramp.slope, ramp.radius, ramp.q]);
    p.table("LOOK", t.look);
    p.table("ENH", t.enhanced);
    let curve = match t.enhanced_curve {
        Some(curve) => p.push(curve.values().iter().copied()),
        None => -1.,
    };
    p.set("ENH_CURVE", &[curve]);
    let tone = p.push(t.tone.iter().flatten().copied());
    p.set("TONE", &[tone]);
    p.set("TONE_COUNT", &[t.tone.len() as f32]);
    p.set("TONE_UNIFORM", &[uniform_knots(t.tone) as u8 as f32]);
    p.set("EXPOSURE_EV", &[r.exposure + r.camera_exposure]);
    p.set("GLOBAL_SH", &[r.shadows, r.highlights]);
    if let Some(local) = &lut.local {
        set_local(&mut p, local);
    }
    let basic = match &lut.basic {
        Some(b) => p.push(b.lut.iter().copied()),
        None => -1.,
    };
    p.set("BASIC", &[basic]);
    let tone = p.push(crate::develop::basic_tone::gpu_tables(&lut.photo.whites));
    p.set("LOCAL_TONE", &[tone]);
    p.set(
        "LOCAL_PIVOT",
        &[match lut.photo.contrast {
            crate::develop::basic_tone::ContrastCurve::Original => -1.,
            crate::develop::basic_tone::ContrastCurve::Pivot(pivot) => pivot,
        }],
    );
    p.set("LEVELS", &[r.black_point, r.white_point, r.midtone]);
    let e = &r.effects;
    // The original per-channel curve runs in `level`, the measured one after it.
    p.set(
        "PARAMETRIC_ON",
        &[(lut.parametric.is_none() && e.parametric != [0.; 4]) as u8 as f32],
    );
    p.set("PARAMETRIC", &e.parametric);
    p.set("SPLITS", &e.splits);
    let parametric = match &lut.parametric {
        Some(c) => p.push(c.values().iter().copied()),
        None => -1.,
    };
    p.set("PARAMETRIC_LUT", &[parametric]);
    // -1 skips the table of a curve that maps each value to itself.
    let straight = crate::color::curve::ToneCurve::is_identity;
    let master = match straight(&r.curve) {
        true => -1.,
        false => p.push(lut.master.values().iter().copied()),
    };
    p.set("MASTER", &[master]);
    p.set("REFINE_SATURATION", &[r.curve_saturation.clamp(0., 1.)]);
    let channels: [f32; 3] = std::array::from_fn(|c| match straight(&r.effects.channels[c]) {
        true => -1.,
        false => p.push(lut.channels[c].values().iter().copied()),
    });
    p.set("CHANNELS", &channels);
    let mixer = match &lut.mixer {
        Some(m) => p.push(m.delta.iter().flatten().copied()),
        None => -1.,
    };
    p.set("MIXER", &[mixer]);
    let gray_grid = match &lut.gray_grid {
        Some(g) => p.push(g.iter().flatten().copied()),
        None => -1.,
    };
    p.set("GRAY_GRID", &[gray_grid]);
    p.set(
        "SATURATION_GRAY",
        &[lut.mixer.as_ref().map_or(0., |m| m.saturation_gray)],
    );
    let gray_source = match lut.mixer.as_ref().and_then(|m| m.gray_source.as_ref()) {
        Some(g) => p.push(g.iter().flatten().copied()),
        None => -1.,
    };
    p.set("GRAY_SOURCE", &[gray_source]);
    let point = match &lut.point_colors {
        Some(pc) => [p.push(pc.params()), pc.len() as f32],
        None => [-1., 0.],
    };
    p.set("POINT", &point);
    set_rgb_table(&mut p, lut.rgb_table.as_ref());
    use crate::develop::color_grade::ColorGrade;
    let grade = match &lut.grade {
        Some(ColorGrade::Luminance(g)) => [
            p.push(g.gain.iter().flatten().copied()),
            p.push(g.offset.iter().flatten().copied()),
            g.gain.len() as f32,
            0.,
        ],
        Some(ColorGrade::Channels(c)) => [
            p.push(c.gain.iter().flatten().copied()),
            -1.,
            c.gain.len() as f32,
            1.,
        ],
        None => [-1., -1., 0., 0.],
    };
    p.set("GRADE", &grade);
    p.set("ADJUST", &[lut.color_adjustments as u8 as f32]);
    p.set("DEFRINGE", &e.defringe);
    p.set("DEFRINGE_RANGES", e.defringe_ranges.as_flattened());
    p.set(
        "GAMUT_CLIP",
        &[(r.gamut_model == crate::model::operators::GamutModel::Clip) as u8 as f32],
    );
    p.set("MONO", &[e.monochrome as u8 as f32]);
    p.set("GRAY_MIX", &e.gray_mix);
    Some(p)
}
impl PixelParams {
    /// Adds the masks' weights and adjustments. `false` when the port cannot render
    /// them: Texture, Clarity, Sharpness and Noise are applied around this stage on
    /// the CPU, and more masks than the parameters hold.
    pub(crate) fn set_masks(
        &mut self,
        im: Source,
        r: &Recipe,
        weights: Option<&crate::develop::masks::MaskWeights>,
    ) -> bool {
        use crate::develop::masks::local;
        let Some(w) = weights else {
            return true;
        };
        let n = w.groups();
        if n == 0 {
            return true;
        }
        if n > crate::model::masks::MAX_GROUPS {
            return false;
        }
        let words = n.div_ceil(4);
        self.set("MASKS", &[n as f32]);
        self.set("MASK_WORDS", &[words as f32]);
        let deltas = self.push(w.deltas.iter().flatten().copied());
        self.set("MASK_DELTAS", &[deltas]);
        let math = local::LocalMath::new(&im.metadata, r);
        self.set("LOCAL_WB", math.white_balance.as_flattened());
        let families = self.push(crate::develop::local_tone::gpu_families());
        self.set("LOCAL_FAMILIES", &[families]);
        let pixels = w.data.len() / n;
        self.weights = vec![0; pixels * words];
        for (i, pixel) in w.data.chunks_exact(n).enumerate() {
            for (k, v) in pixel.iter().enumerate() {
                self.weights[i * words + k / 4] |= u32::from(*v) << (8 * (k % 4));
            }
        }
        true
    }
}
