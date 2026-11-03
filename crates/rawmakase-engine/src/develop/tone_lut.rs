//! A measured tone curve sampled over 0–1, applied as DNG RGBTone does: the global
//! Contrast slider's and the parametric curve's.

const SIZE: usize = 1024;

/// A tone curve sampled at `SIZE + 1` points over 0–1.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ToneLut {
    lut: Vec<f32>,
}
impl ToneLut {
    /// `curve` sampled over 0–1, never falling: measured tables carry small
    /// non-monotone noise, and tone must never invert.
    pub(crate) fn rising(curve: impl Fn(f32) -> f32) -> Self {
        let lut = (0..=SIZE)
            .map(|i| curve(i as f32 / SIZE as f32))
            .scan(0f32, |max, y| {
                *max = max.max(y);
                Some(*max)
            })
            .collect();
        Self { lut }
    }
    /// `first`, then `then`, as one curve.
    pub(crate) fn then(first: Option<Self>, then: Option<Self>) -> Option<Self> {
        match (first, then) {
            (Some(a), Some(b)) => Some(Self {
                lut: a.lut.iter().map(|y| b.eval(*y)).collect(),
            }),
            (a, b) => a.or(b),
        }
    }
    /// The samples, as the GPU port's table.
    pub(crate) fn values(&self) -> &[f32] {
        &self.lut
    }
    pub(crate) fn eval(&self, x: f32) -> f32 {
        let f = x.clamp(0., 1.) * SIZE as f32;
        let i = (f as usize).min(SIZE - 1);
        self.lut[i] + (self.lut[i + 1] - self.lut[i]) * (f - i as f32)
    }
    /// DNG RGBTone: the curve maps the largest and smallest channel, and the middle
    /// channel keeps its relative position, preserving hue.
    pub(crate) fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let p = p.map(|v| v.clamp(0., 1.));
        let lo = p.into_iter().fold(f32::INFINITY, f32::min);
        let hi = p.into_iter().fold(0f32, f32::max);
        let (a, b) = (self.eval(lo), self.eval(hi));
        if hi - lo > 1e-8 {
            p.map(|v| a + (b - a) * (v - lo) / (hi - lo))
        } else {
            [a; 3]
        }
    }
}
