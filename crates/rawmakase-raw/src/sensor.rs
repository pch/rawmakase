//! The decoded sensor data and the levels development reads with it.

/// Unpacked sensor data: `height` rows of `width × channels` samples.
#[derive(Clone, Debug, Default)]
pub struct Sensor {
    pub width: u32,
    pub height: u32,
    /// 1 for a mosaic, 3 for luma and chroma converted to RGB.
    pub channels: u32,
    pub data: Vec<u16>,
    /// The area development reads: left, top, width, height (the visible
    /// area; a Super CCD's stored rotated area).
    pub area: [u32; 4],
    /// Black per channel (red, green, blue, second green), the pattern
    /// included in none of them.
    pub black: [u32; 4],
    /// A black level repeating over `rows × cols` pixels on top of `black`.
    pub black_pattern: (u32, u32, Vec<u32>),
    pub white: u32,
    /// Per-channel linear limits the file records, zero when it has none.
    pub linear_white: [u32; 4],
}
