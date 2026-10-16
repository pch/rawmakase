//! The one policy for black and white levels: from the file, from the
//! camera fact tables, or measured from a masked border.

/// A DNG's BlackLevel over its repeat pattern of `dim` (rows, columns; `None`
/// when the file has no BlackLevelRepeatDim) as per-channel black and the
/// pattern on top of it. A single value is the black of every channel (with
/// an empty one-pixel pattern when the dimension is given); a larger pattern
/// is kept as one, whole values.
pub(crate) fn dng_black(dim: Option<[u32; 2]>, black: &[f64]) -> ([u32; 4], (u32, u32, Vec<u32>)) {
    let value = |i: usize| black.get(i).copied().unwrap_or(0.).max(0.) as u32;
    match dim {
        None => ([value(0); 4], (0, 0, Vec::new())),
        Some([1, 1]) => ([value(0); 4], (1, 1, vec![0])),
        Some([rows, cols]) => {
            let values = (0..(rows * cols) as usize).map(value).collect();
            ([0; 4], (rows, cols, values))
        }
    }
}
