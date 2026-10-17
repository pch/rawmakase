//! The one policy for black and white levels: from the file, from the
//! camera fact tables, or measured from a masked border.

/// A DNG's BlackLevel of whole values over its repeat pattern of `dim`
/// (rows, columns; `None` when the file has no BlackLevelRepeatDim) as
/// per-channel black and the pattern on top of it. A single value is the
/// black of every channel (with an empty one-pixel pattern when the
/// dimension is given); a larger pattern is kept as one. (Other levels are
/// applied to the samples by the DNG reader.)
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

/// The black level of each channel (red, green, blue, second green) measured
/// on the masked columns beside the visible area, over the visible rows: on
/// the left leaving out `trim` columns at either end (the outermost, and
/// those next to the image, which can be lit), and on the right every column
/// when there are at least three; or the `columns` (start, end) the camera
/// table gives. The mean of each channel's samples, to the nearest; `None`
/// without masked columns.
pub(crate) fn masked_black(
    data: &[u16],
    width: usize,
    area: [u32; 4],
    cfa: &crate::info::Cfa,
    columns: Option<[u32; 2]>,
    trim: usize,
) -> Option<[u32; 4]> {
    let [left, top, visible, height] = area.map(|v| v as usize);
    let right = left.checked_add(visible).filter(|&r| r <= width)?;
    let columns: Vec<usize> = match columns {
        Some([start, end]) => (start as usize..(end as usize).min(width)).collect(),
        None => {
            let beyond = if width - right >= 3 {
                right..width
            } else {
                0..0
            };
            (trim..left.saturating_sub(trim)).chain(beyond).collect()
        }
    };
    if columns.is_empty() {
        return None;
    }
    let mut sum = [0u64; 4];
    let mut count = [0u64; 4];
    for y in top..top.saturating_add(height) {
        let row = data.get(y * width..(y + 1) * width)?;
        for &x in &columns {
            // The filter is described from the visible area's corner.
            let c = usize::from(cfa.color(
                y + cfa.rows - top % cfa.rows,
                x + cfa.cols - left % cfa.cols,
            ))
            .min(3);
            sum[c] += u64::from(row[x]);
            count[c] += 1;
        }
    }
    Some(std::array::from_fn(|c| {
        (sum[c] + count[c] / 2).checked_div(count[c]).unwrap_or(0) as u32
    }))
}

/// RAWmakase's correction of a black level far below the left masked
/// border's: the border's median, when the border is dark and even (masked
/// pixels, not image) and the black read (`black`, the mean over the
/// channels) is under a quarter of it. Some bodies' borders sit above their
/// true black (Pentax K-70: 130 against 64), so a black of a quarter of the
/// border or more is kept. Samples every third visible row, skipping the
/// outermost columns and those next to the image, which can be lit.
pub(crate) fn masked_border_black(
    data: &[u16],
    width: usize,
    area: [u32; 4],
    black: u32,
    white: u32,
) -> Option<u32> {
    let [left, top, _, height] = area.map(|v| v as usize);
    if left < 16 {
        return None;
    }
    let mut v = Vec::new();
    for y in (top..top + height).step_by(3) {
        let row = data.get(y * width + 4..y * width + left - 8)?;
        v.extend_from_slice(row);
    }
    let n = v.len();
    if n < 1000 {
        return None;
    }
    v.sort_unstable();
    let (p10, p50, p90) = (
        u32::from(v[n / 10]),
        u32::from(v[n / 2]),
        u32::from(v[n * 9 / 10]),
    );
    if p50 < 64 || p50 > white / 8 || p90 - p10 > 64.max(p50 / 4) || black * 4 >= p50 {
        return None;
    }
    Some(p50)
}

#[cfg(test)]
mod tests {
    use crate::info::Cfa;

    /// A 12 × 4 RGGB sensor (the visible area from column 6 to 10): the
    /// border columns hold 100 + the channel, the outermost and the image
    /// 5000, two right columns 300.
    fn sensor() -> Vec<u16> {
        (0..48)
            .map(|i| {
                let (x, y) = (i % 12, i / 12);
                let channel = [[0, 1], [3, 2]][y % 2][x % 2] as u16;
                match x {
                    2..=3 => 100 + channel,
                    10..=11 => 300,
                    _ => 5000,
                }
            })
            .collect()
    }

    #[test]
    fn masked_black_reads_the_borders_beside_the_image() {
        let cfa = Cfa {
            rows: 2,
            cols: 2,
            colors: vec![0, 1, 3, 2],
        };
        let data = sensor();
        // Columns 2 and 3 only (two left out at either end), no right
        // border of three columns or more.
        assert_eq!(
            super::masked_black(&data, 12, [6, 0, 4, 4], &cfa, None, 2),
            Some([100, 101, 102, 103])
        );
        // The table's columns: the right border.
        assert_eq!(
            super::masked_black(&data, 12, [6, 0, 4, 4], &cfa, Some([10, 12]), 2),
            Some([300; 4])
        );
        // With a right border of three columns (from 9): red from columns 2
        // and 10.
        assert_eq!(
            super::masked_black(&data, 12, [6, 0, 3, 4], &cfa, None, 2).map(|b| b[0]),
            Some(200)
        );
        assert_eq!(
            super::masked_black(&data, 12, [6, 0, 7, 4], &cfa, None, 2),
            None
        );
    }

    #[test]
    fn masked_black_rounds_to_the_nearest() {
        let cfa = crate::info::Cfa {
            rows: 2,
            cols: 2,
            colors: vec![0, 1, 3, 2],
        };
        // Columns 2 and 3 of a 6-column border: each channel reads 100 and
        // 101 on alternate row pairs.
        let data: Vec<u16> = (0..12 * 4)
            .map(|i| if (i / 12) / 2 == 0 { 100 } else { 101 })
            .collect();
        assert_eq!(
            super::masked_black(&data, 12, [6, 0, 6, 4], &cfa, None, 2),
            Some([101; 4])
        );
    }

    #[test]
    fn masked_border_black_corrects_only_a_black_far_below_the_border() {
        let mut data = vec![5000u16; 40 * 400];
        for y in 0..400 {
            for x in 0..30 {
                data[y * 40 + x] = 512 + (x % 3) as u16;
            }
        }
        let area = [30, 0, 10, 400];
        assert_eq!(
            super::masked_border_black(&data, 40, area, 0, 16383),
            Some(513)
        );
        assert_eq!(
            super::masked_border_black(&data, 40, area, 200, 16383),
            None
        );
        // A border too narrow to read.
        assert_eq!(
            super::masked_border_black(&data, 40, [10, 0, 30, 400], 0, 16383),
            None
        );
    }
}
