//! Pixel buffers in shared memory: the descriptor a request carries for each.
//!
//! The pixels never travel in JSON. The app allocates every buffer, in both
//! directions, and passes its handle with the message
//! ([`Handles`](crate::message::Handles)); the descriptor says how to read the
//! mapping, and [`Buffer::handle`] which of the message's handles it is.
//!
//! **Layout.** Samples are little-endian and channels interleaved. Rows run top
//! to bottom, row `r` starting at byte `r * stride`; the bytes after a row's
//! last pixel, up to `stride`, are padding.
//!
//! **Where pixels lie.** [`Region::overlap`] pixels on each side lie outside
//! [`Region::rect`], for tiles; the `width - 2 * overlap` columns between them
//! span the rect exactly, edge to edge. Column `i`'s centre is at
//!
//! `u(i) = x0 + (i - overlap + 0.5) * (x1 - x0) / (width - 2 * overlap)`
//!
//! and row `j`'s likewise with `y0`, `y1` and `height`
//! ([`Region::column_centre`], [`Region::row_centre`]). With no overlap over
//! the whole space this is `u = (i + 0.5) / width`. An overlap pixel whose
//! centre falls beyond the photo's edge holds the edge pixel's value.
use crate::message::{ErrorCode, RemoteError};
use serde::{Deserialize, Serialize};

/// The most bytes one request's buffers may hold together.
pub const MAX_REQUEST_BYTES: u64 = 256 << 20;
/// The most bytes one extension's requests may hold at once, counting requests
/// cancelled but not yet answered.
pub const MAX_EXTENSION_BYTES: u64 = 512 << 20;

/// A pixel buffer's descriptor.
///
/// Unlike a request's parameters, a descriptor refuses fields it doesn't know:
/// a later field may change how the bytes are laid out, and a reader that
/// ignored it would read the wrong pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Buffer {
    /// The index of the buffer's handle among the handles its message carries.
    pub handle: u32,
    /// The mapping's length.
    pub bytes: u64,
    /// Pixels across.
    pub width: u32,
    /// Pixels down.
    pub height: u32,
    /// Bytes from one row's start to the next's.
    pub stride: u32,
    /// Interleaved channels per pixel, 1 to 4.
    pub channels: u8,
    /// How each channel's value is stored.
    pub sample: Sample,
    /// What the values are.
    pub meaning: Meaning,
    /// Where the pixels lie.
    pub region: Region,
    /// Whether the program reads the buffer or writes it.
    pub access: Access,
}

/// How a channel's value is stored, little-endian.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sample {
    /// 0 to 255.
    U8,
    /// 0 to 65535.
    U16,
    /// IEEE 754 single precision. May hold values below 0 and above 1.
    F32,
}
impl Sample {
    /// Bytes per value.
    pub fn size(self) -> u64 {
        match self {
            Self::U8 => 1,
            Self::U16 => 2,
            Self::F32 => 4,
        }
    }
}

/// What a buffer's values are.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Meaning {
    /// How much of each pixel a mask covers, one channel, 0 (none) to the
    /// sample's maximum (all).
    Coverage,
    /// Colour: three channels, or four with alpha.
    Color {
        /// The primaries.
        primaries: Primaries,
        /// The white point.
        white: White,
        /// How values encode light.
        transfer: Transfer,
        /// The fourth channel, if any.
        alpha: Alpha,
    },
}

/// A colour's primaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Primaries {
    /// sRGB and Rec. 709.
    Srgb,
    /// Rec. 2020, the engine's working primaries.
    Rec2020,
    /// ProPhoto RGB.
    ProPhoto,
    /// Adobe RGB (1998).
    AdobeRgb,
    /// The camera's own RGB, before white balance: a raw patch's values.
    Camera,
}

/// A colour's white point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum White {
    /// D65.
    D65,
    /// D50.
    D50,
    /// The camera's, before white balance.
    Camera,
}

/// How a colour's values encode light.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Transfer {
    /// Proportional to light.
    Linear,
    /// The sRGB curve.
    Srgb,
    /// A pure power: the value is light to the power `1 / gamma`.
    Gamma(f32),
}

/// A colour's alpha channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Alpha {
    /// None: three channels.
    None,
    /// A fourth channel, colour not multiplied by it.
    Straight,
    /// A fourth channel, colour already multiplied by it.
    Premultiplied,
}

/// Where a buffer's pixels lie.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    /// The space `rect` is in.
    pub space: Space,
    /// `[x0, y0, x1, y1]`, normalised to the space: 0 to 1, top left first.
    pub rect: [f32; 4],
    /// Buffer pixels per full-resolution pixel of the space.
    pub scale: f32,
    /// Pixels beyond `rect` on each side, for tiles; part of `width` and
    /// `height`.
    pub overlap: u32,
}

/// The space a [`Region`] is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Space {
    /// The photo's image frame: the camera-oriented default crop, before
    /// lens correction, Transform and crop (viewer input's frame).
    ImageFrame,
    /// The developed photo, after Transform, before crop.
    Developed,
    /// The output: the developed photo after crop.
    Output,
    /// The raw image's own frame, before any geometry (lens correction,
    /// Transform, crop), for raw patches.
    Camera,
}

/// Whether the program reads a buffer or writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// The app filled it; the program reads it.
    Read,
    /// The program fills it; the app reads it after the reply.
    Write,
}

impl Buffer {
    /// Checks the descriptor against itself and [`MAX_REQUEST_BYTES`]:
    ///
    /// - at least one pixel, and 1 to 4 channels, as many as its meaning has;
    /// - `stride >= width * channels * sample size`;
    /// - `stride * height <= bytes <= MAX_REQUEST_BYTES`;
    /// - a finite, ordered, non-empty rect in [0, 1], a finite positive scale,
    ///   and overlap leaving at least one pixel of the rect;
    /// - a finite positive gamma.
    ///
    /// Sizes are computed in checked `u64` arithmetic, so nothing a sender
    /// writes can wrap around.
    pub fn validate(&self) -> Result<(), RemoteError> {
        let fail = |why: String| Err(invalid(why));
        if self.width == 0 || self.height == 0 {
            return fail(format!(
                "a buffer of {}x{} pixels is empty",
                self.width, self.height
            ));
        }
        if !(1..=4).contains(&self.channels) {
            return fail(format!(
                "a buffer has {} channels, not 1 to 4",
                self.channels
            ));
        }
        let expected = match self.meaning {
            Meaning::Coverage => 1,
            Meaning::Color {
                alpha: Alpha::None, ..
            } => 3,
            Meaning::Color { .. } => 4,
        };
        if self.channels != expected {
            return fail(format!(
                "a {:?} buffer has {} channels, not {expected}",
                self.meaning, self.channels
            ));
        }
        if let Meaning::Color {
            transfer: Transfer::Gamma(gamma),
            ..
        } = self.meaning
            && !(gamma.is_finite() && gamma > 0.0)
        {
            return fail(format!("a gamma of {gamma} is not positive"));
        }
        let row = u64::from(self.width)
            .checked_mul(u64::from(self.channels))
            .and_then(|n| n.checked_mul(self.sample.size()))
            .ok_or_else(|| invalid("a buffer's row overflows".into()))?;
        if u64::from(self.stride) < row {
            return fail(format!(
                "a stride of {} bytes is shorter than a row of {row}",
                self.stride
            ));
        }
        let needed = u64::from(self.stride)
            .checked_mul(u64::from(self.height))
            .ok_or_else(|| invalid("a buffer's size overflows".into()))?;
        if needed > self.bytes {
            return fail(format!(
                "{} rows of {} bytes don't fit a mapping of {} bytes",
                self.height, self.stride, self.bytes
            ));
        }
        if self.bytes > MAX_REQUEST_BYTES {
            return fail(format!(
                "a buffer of {} bytes is over the {MAX_REQUEST_BYTES}-byte budget",
                self.bytes
            ));
        }
        self.region.validate(self.width, self.height)
    }
}

impl Region {
    /// Where column `i`'s centre lies in the space, normalised, for a buffer
    /// `width` pixels across: inside [`Region::rect`] for the interior
    /// columns, outside it for the overlap. The value may fall outside [0, 1]
    /// for an overlap column past the photo's edge.
    pub fn column_centre(&self, i: u32, width: u32) -> f64 {
        centre(i, width, self.overlap, self.rect[0], self.rect[2])
    }

    /// Where row `j`'s centre lies, as [`Region::column_centre`] does for
    /// columns, for a buffer `height` pixels down.
    pub fn row_centre(&self, j: u32, height: u32) -> f64 {
        centre(j, height, self.overlap, self.rect[1], self.rect[3])
    }

    fn validate(&self, width: u32, height: u32) -> Result<(), RemoteError> {
        let [x0, y0, x1, y1] = self.rect;
        let inside = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
        if !(self.rect.iter().all(|&v| inside(v)) && x0 < x1 && y0 < y1) {
            return Err(invalid(format!(
                "the rect {:?} is not an ordered, non-empty box in [0, 1]",
                self.rect
            )));
        }
        if !(self.scale.is_finite() && self.scale > 0.0) {
            return Err(invalid(format!(
                "a scale of {} is not positive",
                self.scale
            )));
        }
        let margin = u64::from(self.overlap) * 2;
        if margin >= u64::from(width) || margin >= u64::from(height) {
            return Err(invalid(format!(
                "an overlap of {} leaves nothing of a {width}x{height} buffer",
                self.overlap
            )));
        }
        Ok(())
    }
}

/// `lo + (i - overlap + 0.5) * (hi - lo) / (n - 2 * overlap)`, in f64 so the
/// arithmetic neither overflows nor loses a pixel's position on large buffers.
/// [`Buffer::validate`] guarantees `n > 2 * overlap`.
fn centre(i: u32, n: u32, overlap: u32, lo: f32, hi: f32) -> f64 {
    let interior = f64::from(n) - 2.0 * f64::from(overlap);
    let at = f64::from(i) - f64::from(overlap) + 0.5;
    f64::from(lo) + at * (f64::from(hi) - f64::from(lo)) / interior
}

fn invalid(message: String) -> RemoteError {
    RemoteError::new(ErrorCode::InvalidRequest, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// A mask request's input: the photo as 8-bit sRGB, 1280 pixels across.
    fn image() -> Buffer {
        Buffer {
            handle: 0,
            bytes: 1280 * 3 * 960,
            width: 1280,
            height: 960,
            stride: 1280 * 3,
            channels: 3,
            sample: Sample::U8,
            meaning: Meaning::Color {
                primaries: Primaries::Srgb,
                white: White::D65,
                transfer: Transfer::Srgb,
                alpha: Alpha::None,
            },
            region: Region {
                space: Space::ImageFrame,
                rect: [0.0, 0.0, 1.0, 1.0],
                scale: 0.25,
                overlap: 0,
            },
            access: Access::Read,
        }
    }

    fn refused(buffer: &Buffer) -> String {
        let error = buffer.validate().unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        error.message
    }

    #[test]
    fn a_mask_input_reads_in_its_documented_shape() {
        let buffer = image();
        buffer.validate().unwrap();
        let value = serde_json::to_value(&buffer).unwrap();
        assert_eq!(
            value,
            json!({
                "handle": 0, "bytes": 3_686_400u64, "width": 1280, "height": 960,
                "stride": 3840, "channels": 3, "sample": "u8",
                "meaning": {"color": {"primaries": "srgb", "white": "d65",
                                      "transfer": "srgb", "alpha": "none"}},
                "region": {"space": "image_frame", "rect": [0.0, 0.0, 1.0, 1.0],
                           "scale": 0.25, "overlap": 0},
                "access": "read"
            })
        );
        assert_eq!(serde_json::from_value::<Buffer>(value).unwrap(), buffer);
        let coverage: Buffer = serde_json::from_value(json!({
            "handle": 1, "bytes": 1_228_800u64, "width": 1280, "height": 960,
            "stride": 1280, "channels": 1, "sample": "u8", "meaning": "coverage",
            "region": {"space": "image_frame", "rect": [0, 0, 1, 1], "scale": 0.25,
                       "overlap": 0},
            "access": "write"
        }))
        .unwrap();
        coverage.validate().unwrap();
    }

    /// Filters (developed linear Rec. 2020) and raw patches (camera RGB) fit
    /// the descriptor as it is: what they need beyond it travels in their own
    /// request.
    #[test]
    fn filter_and_patch_shaped_buffers_validate() {
        let filter = Buffer {
            handle: 0,
            bytes: 512 * 512 * 3 * 4,
            width: 512,
            height: 512,
            stride: 512 * 3 * 4,
            channels: 3,
            sample: Sample::F32,
            meaning: Meaning::Color {
                primaries: Primaries::Rec2020,
                white: White::D65,
                transfer: Transfer::Linear,
                alpha: Alpha::None,
            },
            region: Region {
                space: Space::Developed,
                rect: [0.25, 0.25, 0.5, 0.5],
                scale: 1.0,
                overlap: 16,
            },
            access: Access::Read,
        };
        let patch = Buffer {
            meaning: Meaning::Color {
                primaries: Primaries::Camera,
                white: White::Camera,
                transfer: Transfer::Linear,
                alpha: Alpha::None,
            },
            region: Region {
                space: Space::Camera,
                rect: [0.4, 0.4, 0.45, 0.45],
                scale: 1.0,
                overlap: 0,
            },
            access: Access::Write,
            ..filter
        };
        for buffer in [filter, patch] {
            buffer.validate().unwrap();
            let line = serde_json::to_string(&buffer).unwrap();
            let read: Buffer = serde_json::from_str(&line).unwrap();
            read.validate().unwrap();
            assert_eq!(read, buffer);
        }
    }

    #[test]
    fn pixel_centres_span_the_rect_and_overlap_lies_outside() {
        let tile = Region {
            space: Space::Developed,
            rect: [0.25, 0.0, 0.75, 1.0],
            scale: 1.0,
            overlap: 1,
        };
        // Four columns, one of overlap each side: two interior columns split
        // [0.25, 0.75] in half and sit at its quarter points.
        let columns: Vec<f64> = (0..4).map(|i| tile.column_centre(i, 4)).collect();
        assert_eq!(columns, [0.125, 0.375, 0.625, 0.875]);
        assert!(columns[0] < 0.25 && columns[3] > 0.75);
        assert!(columns[1..3].iter().all(|u| (0.25..=0.75).contains(u)));
        // Rows of the same tile, over [0, 1] with one row of overlap: the
        // outer rows fall past the photo's edge, where they hold its edge.
        let rows: Vec<f64> = (0..4).map(|j| tile.row_centre(j, 4)).collect();
        assert_eq!(rows, [-0.25, 0.25, 0.75, 1.25]);
        let whole = Region {
            rect: [0.0, 0.0, 1.0, 1.0],
            overlap: 0,
            ..tile
        };
        for i in 0..5 {
            assert_eq!(whole.column_centre(i, 5), (f64::from(i) + 0.5) / 5.0);
        }
    }

    #[test]
    fn sizes_are_checked_without_overflowing() {
        let short = Buffer {
            stride: 1280 * 3 - 1,
            ..image()
        };
        assert!(refused(&short).contains("shorter than a row"));
        let long = Buffer {
            bytes: 1280 * 3 * 960 - 1,
            ..image()
        };
        assert!(refused(&long).contains("don't fit"));
        let over = Buffer {
            width: 16384,
            height: 16384,
            stride: 16384 * 3,
            bytes: 16384 * 3 * 16384,
            ..image()
        };
        assert!(refused(&over).contains("budget"));
        // The largest values a sender can write are refused, not wrapped: the
        // row and the size stay exact in u64.
        let largest = Buffer {
            bytes: u64::MAX,
            width: u32::MAX,
            height: u32::MAX,
            stride: u32::MAX,
            channels: 4,
            sample: Sample::F32,
            meaning: Meaning::Color {
                primaries: Primaries::Srgb,
                white: White::D65,
                transfer: Transfer::Linear,
                alpha: Alpha::Straight,
            },
            ..image()
        };
        assert!(refused(&largest).contains("shorter than a row"));
        let wide = Buffer {
            stride: u32::MAX,
            height: u32::MAX,
            bytes: u64::MAX,
            ..image()
        };
        assert!(refused(&wide).contains("budget"));
        let empty = Buffer {
            width: 0,
            ..image()
        };
        assert!(refused(&empty).contains("empty"));
    }

    #[test]
    fn channels_fit_the_meaning_and_the_region_is_sane() {
        let channels = Buffer {
            channels: 4,
            stride: 1280 * 4,
            bytes: 1280 * 4 * 960,
            ..image()
        };
        refused(&channels);
        let none = Buffer {
            channels: 0,
            ..image()
        };
        refused(&none);
        let coverage = Buffer {
            meaning: Meaning::Coverage,
            ..image()
        };
        refused(&coverage);
        let gamma = Buffer {
            meaning: Meaning::Color {
                primaries: Primaries::AdobeRgb,
                white: White::D65,
                transfer: Transfer::Gamma(f32::NAN),
                alpha: Alpha::None,
            },
            ..image()
        };
        refused(&gamma);
        let region = |rect: [f32; 4], scale: f32, overlap: u32| Buffer {
            region: Region {
                space: Space::ImageFrame,
                rect,
                scale,
                overlap,
            },
            ..image()
        };
        refused(&region([0.0, 0.0, f32::NAN, 1.0], 1.0, 0));
        refused(&region([0.0, 0.0, f32::INFINITY, 1.0], 1.0, 0));
        refused(&region([0.5, 0.0, 0.5, 1.0], 1.0, 0));
        refused(&region([0.0, 0.0, 1.0, 1.5], 1.0, 0));
        refused(&region([0.0, 0.0, 1.0, 1.0], 0.0, 0));
        refused(&region([0.0, 0.0, 1.0, 1.0], f32::INFINITY, 0));
        refused(&region([0.0, 0.0, 1.0, 1.0], 1.0, 480));
        region([0.0, 0.0, 1.0, 1.0], 1.0, 479).validate().unwrap();
    }

    #[test]
    fn unknown_values_and_fields_are_refused() {
        let good = serde_json::to_value(image()).unwrap();
        let with = |path: &[&str], value: Value| {
            let mut changed = good.clone();
            let mut at = &mut changed;
            for key in &path[..path.len() - 1] {
                at = &mut at[*key];
            }
            at[path[path.len() - 1]] = value;
            serde_json::from_value::<Buffer>(changed)
        };
        assert!(with(&["sample"], json!("f16")).is_err());
        assert!(with(&["access"], json!("append")).is_err());
        assert!(with(&["region", "space"], json!("screen")).is_err());
        assert!(with(&["meaning", "color", "primaries"], json!("p3")).is_err());
        assert!(with(&["meaning", "color", "white"], json!("d60")).is_err());
        assert!(with(&["meaning", "color", "transfer"], json!("pq")).is_err());
        assert!(with(&["meaning", "color", "alpha"], json!("mask")).is_err());
        assert!(with(&["meaning"], json!("depth")).is_err());
        assert!(with(&["planar"], json!(true)).is_err());
        assert!(with(&["region", "rotation"], json!(90)).is_err());
        assert!(with(&["meaning", "color", "range"], json!("full")).is_err());
        assert!(with(&["channels"], json!(256)).is_err());
        assert!(with(&["meaning", "color", "transfer"], json!({"gamma": 2.2})).is_ok());
    }
}
