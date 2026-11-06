//! The per-pixel stage's inputs, flattened for the GPU port in `gpu/develop.wgsl`.
//! Recipes with a camera profile are ported (every photo has one once resolved); the
//! rest return `None` and render on the CPU, which stays the reference.
use super::{Camera, CurveSet, Source, camera_stage};
use crate::develop::local_tone::LocalToneMap;
use crate::model::recipe::Recipe;
use std::ops::Range;

/// Named slots of the parameter array and their ranges in it. `wgsl_prelude` turns them
/// into `P_*` index constants for the shader.
pub(crate) mod slot {
    use crate::develop::slots::slots;

    slots! {
        CAMERA: 9,
        // `Camera::clip`, each channel's.
        CAMERA_CLIP: 3,
        WB: 3,
        HUE: 5,
        HUE2: 1,
        HUE_WEIGHT: 1,
        PROFILE_SCALE: 1,
        CALIBRATION: 9,
        SHADOW_TINT: 1,
        EXPOSURE: 1,
        // The scene tone stage's white curves (`GlobalTone::gpu_curves`), its default
        // black and its blacks curve, or -1 each.
        SCENE_WHITE: 1,
        SCENE_DEFAULT_BLACK: 1,
        SCENE_SHADOW_BLACK: 2,
        SCENE_BLACKS: 1,
        // For masks' Whites and Blacks: their curves (`gpu_mask_table`), or -1; and
        // `GlobalTone::gpu_keys`.
        SCENE_TABLES: 1,
        SCENE_KEYS: 6,
        // Dehaze and the level negative Dehaze's response is relative to
        // (`SceneTone::dehaze_key`).
        DEHAZE: 2,
        // Positive Dehaze's haze (`scene_tone::Dehazing`): its density's table, or -1, the
        // grid's size and scale, then the airlight at the render's Exposure.
        HAZE: 5,
        HAZE_AIR: 3,
        LOOK: 5,
        ENH: 5,
        ENH_CURVE: 1,
        TONE: 1,
        TONE_COUNT: 1,
        LOCAL: 1,
        LOCAL_SIZE: 2,
        LOCAL_SCALE: 2,
        LOCAL_A: 3,
        // The fine mean's offset into the grid (`local_tone::MapGrid::fine_at`).
        LOCAL_FINE: 1,
        SHADOWS: 4,
        HIGHLIGHTS: 4,
        // Positive Shadows' detail and region curves (as SHADOWS), and the regional level's
        // grid or -1 (see `local_tone::LocalToneMap::lift`).
        LIFT: 8,
        // The photo's global rule at full strength: positive Shadows' and negative
        // Highlights' curves (as SHADOWS), for masks (`local_tone::LocalToneMap::rule`).
        RULE: 8,
        LOCAL_REGION: 1,
        BASIC: 1,
        LEVELS: 3,
        // The measured parametric curve's table, or -1 (see `parametric::curve`).
        PARAMETRIC_LUT: 1,
        MASTER: 1,
        REFINE_SATURATION: 1,
        CHANNELS: 3,
        MIXER: 1,
        // `black_white::gray_grid`, or -1.
        GRAY_GRID: 1,
        // `ColorMixer::saturation_gray`: how far colors fade to their luminance.
        SATURATION_GRAY: 1,
        // `ColorMixer::gray_source`: its grid, or -1.
        GRAY_SOURCE: 1,
        // Point Color: table offset and number of swatches (see `point_color::params`).
        POINT: 2,
        // A look's RGB table: samples offset, dimensions, divisions, gamma, gamut and
        // amount, then the matrices into its primaries and back.
        RGB: 6,
        RGB_INTO: 9,
        RGB_BACK: 9,
        // Color grading: gain tables and samples (see `color_grade::ColorGrade`).
        GRADE: 2,
        ADJUST: 1,
        DEFRINGE: 2,
        DEFRINGE_RANGES: 4,
        // Masks (see `masks::local`): how many, weight words per pixel, their deltas.
        MASKS: 1,
        MASK_WORDS: 1,
        MASK_DELTAS: 1,
        EXPOSURE_EV: 1,
        LOCAL_WB: 6,
        LOCAL_TONE: 1,
        // The masks' Contrast pivot.
        LOCAL_PIVOT: 1,
        LOCAL_FAMILIES: 1,
        LOCAL_KEYS: 3,
        GLOBAL_SH: 2,
    }
}
pub(crate) fn wgsl_prelude() -> String {
    use crate::develop::point_color::{CONSTANT_PARAMS, SWATCH_PARAMS};
    let fields: String = slot::ALL
        .iter()
        .map(|(name, range)| format!("const P_{name}: u32 = {}u;\n", range.start))
        .collect();
    use crate::develop::scene_tone::{
        MASK_VALUES, SLIDER_STEP, SLIDERS, T_FIRST, T_SAMPLES, T_STEP, U_FIRST, U_SAMPLES, U_STEP,
        Y_FIRST, Y_SAMPLES, Y_STEP,
    };
    fields
        + &format!(
            "const POINT_CONSTANTS: i32 = {CONSTANT_PARAMS};\nconst POINT_SWATCH: i32 = {SWATCH_PARAMS};\n"
        )
        + &format!(
            "const SCENE_T: vec3<f32> = vec3({T_FIRST:?}, {T_STEP:?}, {T_SAMPLES}.0);\n\
             const SCENE_Y: vec3<f32> = vec3({Y_FIRST:?}, {Y_STEP:?}, {Y_SAMPLES}.0);\n\
             const SCENE_U: vec3<f32> = vec3({U_FIRST:?}, {U_STEP:?}, {U_SAMPLES}.0);\n\
             const SCENE_S: vec3<f32> = vec3(-1.0, {SLIDER_STEP:?}, {SLIDERS}.0);\n\
             const MASK_VALUES = array<f32, {}>({});\n\
             const LOCAL_FLOOR: f32 = {:?};\n",
            MASK_VALUES.len(),
            MASK_VALUES.map(|v| format!("{v:?}")).join(", "),
            crate::develop::local_tone::FLOOR
        )
}
pub(crate) struct PixelParams {
    pub(crate) params: Vec<f32>,
    pub(crate) tables: Vec<f32>,
    /// Mask weights, four bytes per word, `MASK_WORDS` words per pixel; empty
    /// without masks.
    pub(crate) weights: Vec<u32>,
    /// The Shadows/Highlights map's grids, which the shader reads from a buffer of their
    /// own (`LOCAL_A`, `LOCAL_FINE` and `LOCAL_REGION` offsets into it; Clarity's into `tables`).
    pub(crate) grid: Option<crate::develop::local_tone::MapGrid>,
    /// The recipe as the camera colour reads it (`stage_cache::color_recipe`), when the
    /// develop pass may keep that colour for its samples and reuse it while the stages
    /// after it change (see `gpu::develop::Kept`). `None` computes it every time.
    pub(crate) color: Option<Recipe>,
}
impl PixelParams {
    /// Parameters of zeros but for `values`, without tables.
    #[cfg(test)]
    pub(crate) fn with(values: &[(Range<usize>, &[f32])]) -> Self {
        let mut p = Self {
            params: vec![0.; slot::HEADER],
            tables: Vec::new(),
            weights: Vec::new(),
            grid: None,
            color: None,
        };
        for (at, v) in values {
            p.set(at.clone(), v);
        }
        p
    }
    /// Sets the slot `at` (one of [`slot`]'s) to `values`, which must fill it.
    fn set(&mut self, at: Range<usize>, values: &[f32]) {
        self.params[at].copy_from_slice(values);
    }
    /// The values of the slot `at` (one of [`slot`]'s).
    pub(crate) fn get(&self, at: Range<usize>) -> &[f32] {
        &self.params[at]
    }
    /// Appends a table and returns its offset as a parameter value.
    fn push(&mut self, values: impl IntoIterator<Item = f32>) -> f32 {
        let at = self.tables.len();
        self.tables.extend(values);
        at as f32
    }
    fn table(&mut self, at: Range<usize>, table: Option<&crate::camera_profiles::Table>) {
        let Some(t) = table else {
            return self.set(at, &[-1., 0., 0., 0., 0.]);
        };
        let offset = self.push(t.data().iter().flatten().copied());
        let [a, b, c] = t.dims();
        self.set(
            at,
            &[offset, a as f32, b as f32, c as f32, t.srgb() as u8 as f32],
        );
    }
}
/// Whether the GPU port renders this (resolved) recipe. It has no HDR rendering: an
/// HDR edit reaches it only as its SDR rendition (`Recipe::resolved`), and an HDR
/// profile's 3D tables, which look up overrange-encoded values
/// (`CameraProfile::camera_color`), render on the CPU.
pub(crate) fn supported(r: &Recipe) -> bool {
    r.profile.as_ref().is_some_and(|p| !p.is_hdr()) && !r.hdr.is_active()
}
/// Parameters for `im`'s per-pixel stage with the resolved recipe `r`, or `None` when
/// the GPU port does not cover it.
pub(crate) fn pixel_params(im: Source, r: &Recipe) -> Option<PixelParams> {
    if !supported(r) {
        return None;
    }
    let camera = camera_stage(&im.metadata, r);
    fill(
        r,
        CurveSet::for_image(im, r, camera, masks_need_map(r)),
        camera,
    )
}
/// Whether active masks change Shadows or Highlights, which read the map.
fn masks_need_map(r: &Recipe) -> bool {
    r.masks
        .iter()
        .any(|m| m.is_active() && (m.adjust.shadows != 0. || m.adjust.highlights != 0.))
}
fn set_local(p: &mut PixelParams, local: &LocalToneMap) {
    p.set(slot::LOCAL, &[1.]);
    p.set(slot::LOCAL_KEYS, &local.keys);
    p.set(slot::LOCAL_SIZE, &[local.width as f32, local.height as f32]);
    p.set(slot::LOCAL_SCALE, &local.scale);
    let grid = local.grid();
    let clarity = match &local.clarity {
        Some(c) => p.push(c.values()),
        None => -1.,
    };
    p.set(slot::LOCAL_A, &[0., grid.cells() as f32, clarity]);
    p.set(slot::LOCAL_FINE, &[grid.fine_at() as f32]);
    p.set(
        slot::LOCAL_REGION,
        &[grid.region_at().map_or(-1., |at| at as f32)],
    );
    p.grid = Some(grid);
    let mut curve = |c: Option<&crate::develop::local_tone::Curve>| match c {
        // The table, then its tail (`local_curve` reads it after the 48 columns).
        Some(c) => [
            p.push(c.table.into_iter().chain([c.tail])),
            c.key,
            c.lo,
            c.hi,
        ],
        None => [-1., 0., 0., 0.],
    };
    let shadows = curve(local.shadows.as_ref());
    let highlights = curve(local.highlights.as_ref());
    let [detail, region] = match &local.lift {
        Some([d, r]) => [curve(Some(d)), curve(Some(r))],
        None => [curve(None), curve(None)],
    };
    let [rule_s, rule_h] = [curve(local.rule[0].as_ref()), curve(local.rule[1].as_ref())];
    p.set(slot::SHADOWS, &shadows);
    p.set(slot::HIGHLIGHTS, &highlights);
    p.set(slot::LIFT, &[detail, region].concat());
    p.set(slot::RULE, &[rule_s, rule_h].concat());
}
fn set_rgb_table(p: &mut PixelParams, look: Option<&crate::camera_profiles::RgbLook>) {
    use crate::camera_profiles::{Dimensions, Gamut};
    let Some(look) = look else {
        return p.set(slot::RGB, &[-1., 0., 0., 0., 0., 0.]);
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
        slot::RGB,
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
    p.set(slot::RGB_INTO, into.as_flattened());
    p.set(slot::RGB_BACK, back.as_flattened());
}
fn fill(r: &Recipe, lut: CurveSet, camera: Camera) -> Option<PixelParams> {
    let profile = r.profile.as_ref()?;
    let mut p = PixelParams {
        params: vec![0.; slot::HEADER],
        tables: Vec::new(),
        weights: Vec::new(),
        grid: None,
        color: None,
    };
    p.set(slot::CAMERA, camera.matrix.as_flattened());
    p.set(slot::CAMERA_CLIP, &camera.clip);
    p.set(slot::WB, &r.wb);
    let t = profile.gpu_tables(r.temperature);
    p.table(slot::HUE, t.hue.map(|h| h.0));
    let hue2 = match t.hue.and_then(|h| h.1) {
        Some(table) => p.push(table.data().iter().flatten().copied()),
        None => -1.,
    };
    p.set(slot::HUE2, &[hue2]);
    p.set(slot::HUE_WEIGHT, &[t.hue.map_or(0., |h| h.2)]);
    p.set(slot::PROFILE_SCALE, &[t.exposure_scale]);
    p.set(slot::CALIBRATION, lut.calibration.matrix.as_flattened());
    p.set(slot::SHADOW_TINT, &[lut.calibration.shadow]);
    p.set(slot::EXPOSURE, &[lut.exposure_gain]);
    let global = &lut.scene.global;
    let white = p.push(global.gpu_curves());
    let default_black = match global.default_black_samples() {
        Some(b) => p.push(b.iter().copied()),
        None => -1.,
    };
    let blacks = match global.blacks_samples() {
        Some(b) => p.push(b.iter().copied()),
        None => -1.,
    };
    p.set(slot::SCENE_WHITE, &[white]);
    p.set(slot::SCENE_DEFAULT_BLACK, &[default_black]);
    let shadow_black = match global.shadow_black_samples() {
        Some((s, rows)) => [p.push(rows.iter().copied()), s],
        None => [-1., 0.],
    };
    p.set(slot::SCENE_SHADOW_BLACK, &shadow_black);
    p.set(slot::SCENE_BLACKS, &[blacks]);
    p.set(slot::SCENE_KEYS, &global.gpu_keys());
    p.set(slot::SCENE_TABLES, &[-1.]);
    p.set(slot::DEHAZE, &[lut.scene.dehaze, lut.scene.dehaze_key]);
    match &lut.scene.haze {
        Some(h) => {
            let density = p.push(h.haze.density.iter().copied());
            p.set(
                slot::HAZE,
                &[
                    density,
                    h.haze.width as f32,
                    h.haze.height as f32,
                    h.scale[0],
                    h.scale[1],
                ],
            );
            p.set(slot::HAZE_AIR, &h.air);
        }
        None => p.set(slot::HAZE, &[-1., 0., 0., 0., 0.]),
    }
    let families = p.push(crate::develop::local_tone::gpu_families());
    p.set(slot::LOCAL_FAMILIES, &[families]);
    p.table(slot::LOOK, t.look);
    p.table(slot::ENH, t.enhanced);
    let curve = match t.enhanced_curve {
        Some(curve) => p.push(curve.values().iter().copied()),
        None => -1.,
    };
    p.set(slot::ENH_CURVE, &[curve]);
    let tone = p.push(t.tone.iter().flatten().copied());
    p.set(slot::TONE, &[tone]);
    p.set(slot::TONE_COUNT, &[t.tone.len() as f32]);
    p.set(slot::EXPOSURE_EV, &[r.exposure + r.camera_exposure]);
    p.set(slot::GLOBAL_SH, &[r.shadows, r.highlights]);
    if let Some(local) = &lut.local {
        set_local(&mut p, local);
    }
    let basic = match &lut.basic {
        Some(b) => p.push(b.values().iter().copied()),
        None => -1.,
    };
    p.set(slot::BASIC, &[basic]);
    let tone = p.push(crate::develop::basic_tone::gpu_tables());
    p.set(slot::LOCAL_TONE, &[tone]);
    p.set(slot::LOCAL_PIVOT, &[lut.photo.contrast_pivot]);
    p.set(slot::LEVELS, &[r.black_point, r.white_point, r.midtone]);
    let e = &r.effects;
    let parametric = match &lut.parametric {
        Some(c) => p.push(c.values().iter().copied()),
        None => -1.,
    };
    p.set(slot::PARAMETRIC_LUT, &[parametric]);
    let master = p.push(lut.master.values().iter().copied());
    p.set(slot::MASTER, &[master]);
    p.set(slot::REFINE_SATURATION, &[r.curve_saturation.clamp(0., 1.)]);
    let channels = lut
        .channels
        .each_ref()
        .map(|c| p.push(c.values().iter().copied()));
    p.set(slot::CHANNELS, &channels);
    let mixer = match &lut.mixer {
        Some(m) => p.push(m.delta.iter().flatten().copied()),
        None => -1.,
    };
    p.set(slot::MIXER, &[mixer]);
    let gray_grid = match &lut.gray_grid {
        Some(g) => p.push(g.iter().flatten().copied()),
        None => -1.,
    };
    p.set(slot::GRAY_GRID, &[gray_grid]);
    p.set(
        slot::SATURATION_GRAY,
        &[lut.mixer.as_ref().map_or(0., |m| m.saturation_gray)],
    );
    let gray_source = match lut.mixer.as_ref().and_then(|m| m.gray_source.as_ref()) {
        Some(g) => p.push(g.iter().flatten().copied()),
        None => -1.,
    };
    p.set(slot::GRAY_SOURCE, &[gray_source]);
    let point = match &lut.point_colors {
        Some(pc) => [p.push(pc.params()), pc.len() as f32],
        None => [-1., 0.],
    };
    p.set(slot::POINT, &point);
    set_rgb_table(&mut p, lut.rgb_table.as_ref());
    let grade = match &lut.grade {
        Some(g) => [
            p.push(g.0.gain.iter().flatten().copied()),
            g.0.gain.len() as f32,
        ],
        None => [-1., 0.],
    };
    p.set(slot::GRADE, &grade);
    p.set(slot::ADJUST, &[lut.color_adjustments as u8 as f32]);
    p.set(slot::DEFRINGE, &e.defringe);
    p.set(slot::DEFRINGE_RANGES, e.defringe_ranges.as_flattened());
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
        // A mask's Clarity and Texture render on the CPU (`scene_stage`, `detail`).
        if n > crate::model::masks::MAX_GROUPS
            || w.deltas
                .iter()
                .any(|d| local::uses(d, &[local::slot::CLARITY, local::slot::TEXTURE]))
        {
            return false;
        }
        let words = n.div_ceil(4);
        self.set(slot::MASKS, &[n as f32]);
        self.set(slot::MASK_WORDS, &[words as f32]);
        let deltas = self.push(w.deltas.iter().flatten().copied());
        self.set(slot::MASK_DELTAS, &[deltas]);
        let math = local::LocalMath::new(&im.metadata, r);
        self.set(slot::LOCAL_WB, math.white_balance.as_flattened());
        if w.deltas.iter().any(|d| local::uses(d, &local::SCENE_SLOTS)) {
            let masks = self.push(crate::develop::scene_tone::gpu_mask_table());
            self.set(slot::SCENE_TABLES, &[masks]);
        }
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
