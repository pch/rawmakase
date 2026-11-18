//! The decoded sensor data and the levels development reads with it.

/// Unpacked sensor data: `height` rows of `width × channels` samples.
#[derive(Clone, Debug, Default)]
pub struct Sensor {
    /// Pixels per row: the stored sensor data's width.
    pub width: u32,
    /// Rows.
    pub height: u32,
    /// 1 for a mosaic, 3 for luma and chroma converted to RGB.
    pub channels: u32,
    /// The samples, row by row, `channels` per pixel.
    pub data: SensorData,
    /// The area development reads: left, top, width, height (the visible
    /// area; a Super CCD's stored rotated area).
    pub area: [u32; 4],
    /// Black per channel (red, green, blue, second green), the pattern
    /// included in none of them.
    pub black: [u32; 4],
    /// A black level repeating over `rows × cols` pixels on top of `black`.
    pub black_pattern: (u32, u32, Vec<u32>),
    /// The level a sample clips at, which development scales to 1.
    pub white: u32,
    /// Per-channel linear limits the file records, zero when it has none.
    pub linear_white: [u32; 4],
}

/// The samples of [`Sensor`], on the scale of its levels.
#[derive(Clone, Debug, PartialEq)]
pub enum SensorData {
    /// Whole-number samples: every format but floating-point DNGs.
    U16(Vec<u16>),
    /// Floating-point samples (DNGs of SampleFormat 3: HDR merges). They
    /// are linear reference values: black 0 and white 1 (a [`Sensor`] of them
    /// has those levels), and each is clipped to 0–1, as DNG's processing
    /// model maps raw values. No precision is lost to a 16-bit scale, so a
    /// merge's shadows keep their gradations however far below white they are.
    F32(Vec<f32>),
}

impl Default for SensorData {
    fn default() -> Self {
        Self::U16(Vec::new())
    }
}

impl From<Vec<u16>> for SensorData {
    fn from(v: Vec<u16>) -> Self {
        Self::U16(v)
    }
}

impl SensorData {
    /// The number of samples.
    pub fn len(&self) -> usize {
        match self {
            Self::U16(v) => v.len(),
            Self::F32(v) => v.len(),
        }
    }
    /// Whether there are no samples.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The whole-number samples, test code's way to read them; panics on
    /// floating-point ones.
    #[cfg(test)]
    pub(crate) fn u16s(&self) -> &[u16] {
        match self {
            Self::U16(v) => v,
            Self::F32(_) => panic!("floating-point sensor data"),
        }
    }
    /// The floating-point samples, test code's way to read them; panics on
    /// whole-number ones.
    #[cfg(test)]
    pub(crate) fn f32s(&self) -> &[f32] {
        match self {
            Self::F32(v) => v,
            Self::U16(_) => panic!("whole-number sensor data"),
        }
    }
}
