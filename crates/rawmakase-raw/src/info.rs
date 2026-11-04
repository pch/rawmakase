//! What opening a RAW file says, before its sensor data is decoded.
use crate::variant::Variant;

/// A crop position that is not known.
pub const UNSET: u32 = 0xffff;

/// The colour space a camera rendered its embedded JPEGs in, as its maker notes
/// say (Nikon's ColorSpace, Olympus's CameraSettings ColorSpace), where the EXIF
/// may not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreviewSpace {
    /// The camera's sRGB setting.
    Srgb,
    /// The camera's Adobe RGB (1998) setting.
    AdobeRgb,
}

/// Where the colour matrix in [`RawInfo::cam_xyz`] comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MatrixSource {
    /// None: the camera table has no row and the file holds none.
    #[default]
    Absent,
    /// The camera table's row from Adobe DNG Converter's output.
    AdobeDng,
    /// The camera table's row from RAWmakase's earlier decoder, for a camera
    /// Adobe has no matrix for.
    CompatObserved,
    /// The DNG's own ColorMatrix tags (or a table rule from them).
    File,
    /// The DNG's own ColorMatrix, of a tungsten calibration alone.
    FileTungstenOnly,
    /// A matrix the camera's maker notes give (Olympus), with its D65 levels.
    MakerNote,
    /// A monochrome DNG's neutral colour, by definition.
    Neutral,
}

/// A RAW file's header facts: identity, geometry, levels' sources, colour and
/// exposure. Names, sizes, crops and levels follow the conventions RAWmakase
/// has always read them with, so saved edits keep their geometry and scale.
#[derive(Clone, Debug)]
pub struct RawInfo {
    /// How the sensor data is stored, which says how it decodes.
    pub variant: Variant,
    /// RAWmakase's name for the camera's maker ("Canon").
    pub make: String,
    /// RAWmakase's name for the model ("EOS R6 Mark II").
    pub model: String,
    /// The make and model as the file writes them, trimmed, before any of
    /// RAWmakase's naming (`names::apply`): what export file names use.
    pub file_make: String,
    /// The model, as the file writes it.
    pub file_model: String,
    /// The canonical name of a body sold under several names (empty make
    /// when there is none).
    pub normalized_make: String,
    /// The canonical model name, with [`RawInfo::normalized_make`].
    pub normalized_model: String,
    /// The stored sensor data's size.
    pub raw_width: u32,
    /// The stored sensor data's height.
    pub raw_height: u32,
    /// The visible area, `margins` (left, top) into the sensor data.
    pub width: u32,
    /// The visible area's height.
    pub height: u32,
    /// Where the visible area starts in the sensor data: left, top.
    pub margins: [u32; 2],
    /// A Fujifilm Super CCD's stored rows, which develop turned by 45° (the
    /// visible area is the stored one); `None` for other sensors.
    pub super_ccd: Option<SuperCcd>,
    /// The default crop and the camera's aspect-ratio crop, `[left, top,
    /// width, height]` in sensor coordinates ([`UNSET`] positions when only a
    /// size is known; all [`UNSET`] and zero sizes when there is none).
    pub frame: [u32; 4],
    /// The camera's aspect-ratio crop, as [`RawInfo::frame`] gives the default.
    pub aspect: [u32; 4],
    /// How much smaller than the sensor's the frame's diagonal is, when the camera
    /// recorded only part of its sensor in a crop mode (Canon's APS-C and APS-H
    /// modes); `None` for the whole sensor or when not known.
    pub sensor_crop: Option<f32>,
    /// Panasonic's sensor borders (RW2 tags 4 to 7), `[left, top, width,
    /// height]` in sensor coordinates, when the default crop is the camera's
    /// aspect ratio inside them.
    pub sensor_borders: Option<[u32; 4]>,
    /// The crop a Fujifilm RAF header recommends, `[left, top, width,
    /// height]` in sensor coordinates.
    pub recommended_crop: Option<[u32; 4]>,
    /// EXIF orientation as a quarter-turn code: 0 none, 3 half turn, 5 a
    /// quarter turn counter-clockwise, 6 clockwise.
    pub flip: i32,
    /// The colour filter over the visible area; `None` for small RAWs and
    /// other data that is not a mosaic.
    pub cfa: Option<Cfa>,
    /// The DNG version, 0 for a camera's own format.
    pub dng_version: u32,
    /// Canon sRAW and mRAW, Nikon sRAW, Sony YCbCr: stored as luma and chroma.
    pub sraw: bool,
    /// Small RAWs the camera stored white balanced (Nikon sRAW, Sony YCbCr):
    /// developed without the as-shot multipliers, which Adobe DNG Converter
    /// writes as their AnalogBalance, with an AsShotNeutral of 1, 1, 1.
    pub balanced: bool,
    /// A linear raw DNG's AnalogBalance relative to green: gains its writer
    /// applied to the samples. `cam_mul` and `pre_mul` are of the samples
    /// with them; XYZ to the samples is these gains times `cam_xyz` (DNG
    /// Specification, "Mapping Camera Color Space to CIE XYZ Space"). 1
    /// otherwise.
    pub analog_balance: [f32; 3],
    /// A monochrome DNG (linear raw of one sample: monochrome sensors, film
    /// scanners): its one sample is every channel's, and its colour neutral
    /// (`color::apply`).
    pub monochrome: bool,
    /// Fujifilm's DevelopmentDynamicRange in percent (400 for DR400), 0xffff when
    /// there is none.
    pub fuji_dynamic_range: u32,
    /// Fujifilm's exposure midpoint shift in EV, -999 when there is none.
    pub fuji_exposure_shift: f32,
    /// ISO speed, 0 when unknown.
    pub iso: f32,
    /// Exposure time in seconds, 0 when unknown.
    pub shutter: f32,
    /// The f-number, 0 when unknown.
    pub aperture: f32,
    /// Focal length in millimetres, 0 when unknown.
    pub focal: f32,
    /// Focal length in 35 mm equivalent, 0 when unknown.
    pub focal_35mm: f32,
    /// As-shot white balance multipliers (red, green, blue, second green);
    /// zero when the file has none.
    pub cam_mul: [f32; 4],
    /// Daylight multipliers derived from the colour matrix.
    pub pre_mul: [f32; 4],
    /// The white balance presets the camera stored.
    pub presets: Presets,
    /// XYZ (D65) to camera, rows red, green, blue; zero when unknown.
    pub cam_xyz: [[f32; 3]; 3],
    /// Where `cam_xyz` comes from.
    pub matrix_source: MatrixSource,
    /// The colour space of the camera's JPEGs, when the maker notes say it.
    pub preview_space: Option<PreviewSpace>,
    /// The look the camera's JPEGs were made with, by the maker's name for it: a
    /// Fujifilm film simulation, Sony Creative Style, Nikon Picture Control or Canon
    /// Picture Style.
    pub camera_look: Option<String>,
    /// Camera to sRGB primaries (rows sum to one).
    pub rgb_cam: [[f32; 3]; 3],
    /// The lens model as the file records it, empty when it records none.
    pub lens: String,
    /// The maker's numeric lens ID (Nikon's LensIDNumber, Pentax's LensType).
    pub lens_id: u32,
    /// The lens's focal range and maximum apertures, as the maker notes give them
    /// (Nikon's Lens tag): shortest and longest focal length, aperture at each.
    pub lens_info: Option<[f32; 4]>,
    /// Canon Highlight Tone Priority: 0 off, 1 on, 2 enhanced.
    pub highlight_tone_priority: i32,
    /// The sensor data's mean black, its white and the samples' maximum
    /// (their bit depth's), where the file says them before unpacking
    /// (Canon's CR2 and CR3 mosaics, whose white Adobe takes by ISO speed):
    /// Adobe DNG Converter's BaselineExposure moves with that white, so
    /// development's exposure follows it.
    pub levels: Option<[u32; 3]>,
    pub(crate) plan: crate::variant::Plan,
    /// The embedded JPEGs the file lists.
    pub(crate) previews: Vec<crate::preview::Candidate>,
    /// Whether the file has a preview that is not JPEG (an uncompressed
    /// thumbnail, an HEVC image).
    pub(crate) uncompressed_preview: bool,
    /// A DNG's AsShotWhiteXY, when it gives the white balance so.
    pub(crate) as_shot_xy: Option<[f64; 2]>,
    /// The XYZ (D65) to camera matrix the file itself holds (a DNG's), and
    /// where in the file it comes from.
    pub(crate) file_matrix: Option<[[f32; 3]; 3]>,
    pub(crate) file_matrix_source: MatrixSource,
    /// The canonical name the file itself gives (Hasselblad's sensor
    /// names), where the camera table has none.
    pub(crate) canonical: Option<[String; 2]>,
    /// The image size the camera records for the shot (EXIF's
    /// PixelXDimension and PixelYDimension), width then height.
    pub(crate) image_size: Option<[u32; 2]>,
}

/// How a Fujifilm Super CCD's stored rows develop: each a diagonal of the
/// image, which is the stored area turned by 45° into a square of the
/// shorter side plus half the longer one, less a row (our own measurements
/// of the images RAWmakase has always developed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuperCcd {
    /// The stored (visible) area's width and height.
    pub stored: [u32; 2],
    /// Rows of the wide layout (FujiLayout 0 to 3, two stored columns per
    /// step along a diagonal) rather than the tall one (0x80 to 0x83, two
    /// stored rows per step across diagonals).
    pub wide: bool,
}

impl SuperCcd {
    /// The developed image's width and height.
    pub fn size(&self) -> [u32; 2] {
        let [w, h] = self.stored;
        let side = w.min(h) + w.max(h) / 2;
        [side, side.saturating_sub(1)]
    }
    /// Where the stored area's `row` and `col` develop (column, row), when
    /// inside the developed image.
    pub fn position(&self, row: u32, col: u32) -> Option<[u32; 2]> {
        let [w, _] = self.stored.map(i64::from);
        let (r, c) = (i64::from(row), i64::from(col));
        let (x, y) = if self.wide {
            ((c + 1) / 2 + r, w / 2 - 1 - c / 2 + r)
        } else {
            (c + (r + 1) / 2, w - 1 - c + r / 2)
        };
        let [sw, sh] = self.size().map(i64::from);
        ((0..sw).contains(&x) && (0..sh).contains(&y)).then_some([x as u32, y as u32])
    }
}

/// White balance presets the camera stored (red, green, blue), zero when
/// absent. Each field is the preset of its name.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Presets {
    pub daylight: [f32; 3],
    pub shade: [f32; 3],
    pub d65: [f32; 3],
    pub fine_weather: [f32; 3],
}

/// A colour filter pattern over the visible area: `colors[r * cols + c]` is
/// the filter at visible row `r`, column `c` modulo the period (0 red, 1
/// green, 2 blue, 3 the second green of a Bayer pattern).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cfa {
    /// The pattern's period in rows.
    pub rows: usize,
    /// The pattern's period in columns.
    pub cols: usize,
    /// `rows × cols` filters, row by row.
    pub colors: Vec<u8>,
}

/// A `RawInfo` of `variant` with nothing else known: no crops, no colour,
/// LibRaw's conventions for what is absent.
pub(crate) fn base(variant: Variant) -> RawInfo {
    RawInfo {
        variant,
        make: String::new(),
        model: String::new(),
        sensor_crop: None,
        file_make: String::new(),
        file_model: String::new(),
        normalized_make: String::new(),
        normalized_model: String::new(),
        raw_width: 0,
        raw_height: 0,
        width: 0,
        height: 0,
        margins: [0, 0],
        super_ccd: None,
        frame: [UNSET, UNSET, 0, 0],
        aspect: [UNSET, UNSET, 0, 0],
        sensor_borders: None,
        recommended_crop: None,
        flip: 0,
        cfa: None,
        dng_version: 0,
        sraw: false,
        balanced: false,
        analog_balance: [1.; 3],
        monochrome: false,
        fuji_dynamic_range: 0xffff,
        fuji_exposure_shift: -999.,
        iso: 0.,
        shutter: 0.,
        aperture: 0.,
        focal: 0.,
        focal_35mm: 0.,
        cam_mul: [0.; 4],
        pre_mul: [0.; 4],
        presets: Presets::default(),
        cam_xyz: [[0.; 3]; 3],
        matrix_source: MatrixSource::Absent,
        preview_space: None,
        camera_look: None,
        rgb_cam: [[0.; 3]; 3],
        lens: String::new(),
        lens_id: 0,
        lens_info: None,
        highlight_tone_priority: 0,
        levels: None,
        plan: crate::variant::Plan::None,
        previews: Vec::new(),
        uncompressed_preview: false,
        as_shot_xy: None,
        file_matrix: None,
        file_matrix_source: MatrixSource::Absent,
        canonical: None,
        image_size: None,
    }
}

/// `cfa` with the green that shares a row with blue marked as the second
/// green (3), as RAWmakase's levels and fingerprints name the channels.
pub(crate) fn second_green(mut cfa: Cfa) -> Cfa {
    for r in 0..cfa.rows {
        let row = &mut cfa.colors[r * cfa.cols..(r + 1) * cfa.cols];
        if row.contains(&2) && !row.contains(&0) {
            for c in row.iter_mut().filter(|c| **c == 1) {
                *c = 3;
            }
        }
    }
    cfa
}

impl Cfa {
    /// The same filter seen from `top`, `left` into it.
    pub fn shifted(&self, top: usize, left: usize) -> Cfa {
        let colors = (0..self.rows)
            .flat_map(|r| (0..self.cols).map(move |c| (r, c)))
            .map(|(r, c)| self.color(r + top, c + left))
            .collect();
        Cfa {
            rows: self.rows,
            cols: self.cols,
            colors,
        }
    }
    /// The filter at visible row `row`, column `col`.
    pub fn color(&self, row: usize, col: usize) -> u8 {
        self.colors[(row % self.rows) * self.cols + col % self.cols]
    }
    /// Whether this is X-Trans's 6 × 6 pattern.
    pub fn is_xtrans(&self) -> bool {
        self.rows == 6 && self.cols == 6
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every stored pixel lands on its own developed pixel, the turned area
    /// is a diamond without holes, and the filters there form a Bayer
    /// pattern (the RAF reader's Super CCD filters, green as 1).
    #[test]
    fn super_ccd_turns_into_a_bayer_diamond() {
        let tall = [0u8, 2, 1, 1, 2, 0, 1, 1];
        let wide = [0u8, 1, 2, 1, 2, 1, 0, 1];
        for (stored, wide_layout, filter, rows, cols) in [
            ([8u32, 12], false, &tall, 4, 2),
            ([12, 6], true, &wide, 2, 4),
        ] {
            let turn = SuperCcd {
                stored,
                wide: wide_layout,
            };
            let [tw, th] = turn.size();
            let side = stored[0].min(stored[1]) + stored[0].max(stored[1]) / 2;
            assert_eq!([tw, th], [side, side - 1]);
            let mut colour = vec![u8::MAX; (tw * th) as usize];
            for r in 0..stored[1] {
                for c in 0..stored[0] {
                    let [x, y] = turn.position(r, c).unwrap();
                    let i = (y * tw + x) as usize;
                    assert_eq!(colour[i], u8::MAX, "{r} {c} twice");
                    colour[i] = filter[(r as usize % rows) * cols + c as usize % cols];
                }
            }
            // Each developed row's stored pixels are one run.
            for y in 0..th {
                let row = &colour[(y * tw) as usize..((y + 1) * tw) as usize];
                let set: Vec<usize> = (0..row.len()).filter(|&x| row[x] != u8::MAX).collect();
                if let (Some(a), Some(b)) = (set.first(), set.last()) {
                    assert_eq!(b - a + 1, set.len(), "row {y}");
                }
            }
            // A Bayer pattern: period 2 both ways, two greens per period.
            let mut period = [u8::MAX; 4];
            for y in 0..th {
                for x in 0..tw {
                    let c = colour[(y * tw + x) as usize];
                    if c == u8::MAX {
                        continue;
                    }
                    let p = &mut period[((y % 2) * 2 + x % 2) as usize];
                    assert!(*p == u8::MAX || *p == c, "{y} {x}");
                    *p = c;
                }
            }
            let mut sorted = period;
            sorted.sort();
            assert_eq!(sorted, [0, 1, 1, 2]);
        }
    }
}
