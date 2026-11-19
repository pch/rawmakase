//! The pixels of a [`CameraImage`](super::CameraImage), and how two images that
//! differ in few pixels hold one buffer between them.
use rayon::prelude::*;
use std::{
    borrow::Cow,
    ops::Range,
    sync::{Arc, OnceLock},
};

/// A camera image's pixels, row by row: a slice of them wherever one is read
/// (`image.pixels.iter()`, `&image.pixels`), made from a `Vec` with `into()` or
/// by collecting an iterator.
///
/// Most images own their buffer, and a copy of one (`clone`) is a buffer of its
/// own. A decode and its highlight-recovered image are the exception: recovery
/// changes few pixels of a RAW (3% in the median photo measured, #1173) and none
/// of a rendered file, so the two hold one buffer between them
/// ([`Pixels::pair`], [`Pixels::share`]). The buffer is the recovered image's,
/// which every render samples; the decode keeps its own values for the pixels
/// that differ.
///
/// Such a decode reads as any other image does. One pixel (`pixels[i]`) or a
/// range of them ([`Pixels::range`]) costs no copy of the image. Reading it as
/// one slice copies the whole image out the first time and keeps the copy: the
/// values are right, but the memory the pair saved is spent again, so code that
/// reads a whole decode uses the first two.
#[derive(Clone)]
pub struct Pixels(Repr);

#[derive(Clone)]
enum Repr {
    /// A buffer of this image's own.
    Own(Vec<[f32; 3]>),
    /// A buffer other images hold too.
    Shared(Arc<Vec<[f32; 3]>>),
    /// Another image's buffer, and this image's values where it differs.
    Patch(Arc<Patch>),
}

struct Patch {
    /// The other image's pixels.
    base: Arc<Vec<[f32; 3]>>,
    /// One bit a pixel, 64 pixels a word from the lowest bit: set where this
    /// image differs from `base`.
    differs: Vec<u64>,
    /// For each word of `differs`, how many pixels before it differ, and after
    /// the last word, how many differ in all.
    before: Vec<usize>,
    /// This image's values where it differs, in pixel order.
    values: Vec<[f32; 3]>,
    /// The whole image, copied out the first time it is read as one slice.
    whole: OnceLock<Vec<[f32; 3]>>,
}

impl Patch {
    /// How many of the pixels before pixel `i` differ; `i` may be the length.
    fn differing_before(&self, i: usize) -> usize {
        let (word, bit) = (i / 64, i % 64);
        match bit {
            0 => self.before[word],
            _ => self.before[word] + (self.differs[word] & ((1 << bit) - 1)).count_ones() as usize,
        }
    }
    fn at(&self, i: usize) -> &[f32; 3] {
        if let Some(whole) = self.whole.get() {
            return &whole[i];
        }
        match self.differs.get(i / 64) {
            Some(word) if (word >> (i % 64)) & 1 == 1 => &self.values[self.differing_before(i)],
            _ => &self.base[i],
        }
    }
    /// The pixels of `range` that differ, in order, with the position of each
    /// one's value in `values`.
    fn differing(&self, range: Range<usize>) -> impl Iterator<Item = (usize, usize)> + '_ {
        let first = self.differing_before(range.start);
        let words = range.start / 64..range.end.div_ceil(64);
        words
            .flat_map(move |word| {
                let mut bits = self.differs[word];
                std::iter::from_fn(move || {
                    (bits != 0).then(|| {
                        let i = word * 64 + bits.trailing_zeros() as usize;
                        bits &= bits - 1;
                        i
                    })
                })
            })
            .filter(move |i| range.contains(i))
            .enumerate()
            .map(move |(k, i)| (i, first + k))
    }
    fn copy(&self, range: Range<usize>) -> Vec<[f32; 3]> {
        let mut out = self.base[range.clone()].to_vec();
        for (i, value) in self.differing(range.clone()) {
            out[i - range.start] = self.values[value];
        }
        out
    }
    fn whole(&self) -> &[[f32; 3]] {
        self.whole.get_or_init(|| self.copy(0..self.base.len()))
    }
}

impl Pixels {
    pub fn len(&self) -> usize {
        match &self.0 {
            Repr::Own(pixels) => pixels.len(),
            Repr::Shared(pixels) => pixels.len(),
            Repr::Patch(patch) => patch.base.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The pixels as a `Vec` of their own: this image's buffer when no other
    /// image holds it, else a copy.
    pub fn into_vec(self) -> Vec<[f32; 3]> {
        match self.0 {
            Repr::Own(pixels) => pixels,
            Repr::Shared(pixels) => Arc::try_unwrap(pixels).unwrap_or_else(|p| (*p).clone()),
            Repr::Patch(patch) => match Arc::try_unwrap(patch) {
                Ok(mut patch) => match patch.whole.take() {
                    Some(whole) => whole,
                    None => patch.copy(0..patch.base.len()),
                },
                // Not through `whole`, which would leave the other holders a copy.
                Err(patch) => match patch.whole.get() {
                    Some(whole) => whole.clone(),
                    None => patch.copy(0..patch.base.len()),
                },
            },
        }
    }
    /// Pixels `range`, without copying the whole image out: borrowed, or for an
    /// image kept as another's buffer that differs within `range`, a copy of
    /// that range alone.
    pub fn range(&self, range: Range<usize>) -> Cow<'_, [[f32; 3]]> {
        match &self.0 {
            Repr::Own(pixels) => Cow::Borrowed(&pixels[range]),
            Repr::Shared(pixels) => Cow::Borrowed(&pixels[range]),
            Repr::Patch(patch) => match patch.whole.get() {
                Some(whole) => Cow::Borrowed(&whole[range]),
                None if patch.differing_before(range.start)
                    == patch.differing_before(range.end) =>
                {
                    Cow::Borrowed(&patch.base[range])
                }
                None => Cow::Owned(patch.copy(range)),
            },
        }
    }
    /// Lets copies of this image (`clone`) hold its buffer with it, where a
    /// copy otherwise has a buffer of its own: for an image whose derived image
    /// has the same pixels (a rendered file and its recovered image). A copy
    /// that is written to takes a buffer of its own first.
    pub fn share(&mut self) {
        if let Repr::Own(pixels) = &mut self.0 {
            self.0 = Repr::Shared(Arc::new(std::mem::take(pixels)));
        }
    }
    /// Two images' pixels held as one buffer: the second image's are `buffer`,
    /// and the first image's are the same except at `changes`, each a pixel's
    /// index and the first image's value there, in ascending order of index.
    /// `None` when `changes` are not ascending or not within `buffer`.
    pub fn pair(
        buffer: Vec<[f32; 3]>,
        changes: impl IntoIterator<Item = (usize, [f32; 3])>,
    ) -> Option<(Self, Self)> {
        let changes = changes.into_iter();
        let mut differs = vec![0u64; buffer.len().div_ceil(64)];
        let mut values = Vec::with_capacity(changes.size_hint().0);
        let mut next = 0;
        for (i, value) in changes {
            if i < next || i >= buffer.len() {
                return None;
            }
            differs[i / 64] |= 1u64 << (i % 64);
            values.push(value);
            next = i + 1;
        }
        Self::pair_masked(buffer, differs, values)
    }
    /// [`Pixels::pair`] from the changes as they are kept, for a caller that makes
    /// them on several threads: `differs` has one bit a pixel, 64 pixels a word
    /// from the lowest bit, set where the first image differs from `buffer`, and
    /// `values` are the first image's pixels there, in pixel order. `None` when
    /// the mask is not the buffer's length, marks a pixel past its end, or
    /// marks another number of pixels than there are values.
    pub fn pair_masked(
        buffer: Vec<[f32; 3]>,
        differs: Vec<u64>,
        mut values: Vec<[f32; 3]>,
    ) -> Option<(Self, Self)> {
        let past_end = match buffer.len() % 64 {
            0 => 0,
            bits => differs.last().map_or(0, |word| word >> bits),
        };
        if differs.len() != buffer.len().div_ceil(64) || past_end != 0 {
            return None;
        }
        let mut before = Vec::with_capacity(differs.len() + 1);
        let mut count = 0;
        for word in &differs {
            before.push(count);
            count += word.count_ones() as usize;
        }
        before.push(count);
        if count != values.len() {
            return None;
        }
        let base = Arc::new(buffer);
        if values.is_empty() {
            return Some((Self(Repr::Shared(base.clone())), Self(Repr::Shared(base))));
        }
        values.shrink_to_fit();
        let patch = Patch {
            base: base.clone(),
            differs,
            before,
            values,
            whole: OnceLock::new(),
        };
        Some((Self(Repr::Patch(Arc::new(patch))), Self(Repr::Shared(base))))
    }
    /// The buffer this image holds with others, while it has no copy of its own.
    fn held(&self) -> Option<&Arc<Vec<[f32; 3]>>> {
        match &self.0 {
            Repr::Own(_) => None,
            Repr::Shared(pixels) => Some(pixels),
            Repr::Patch(patch) => patch.whole.get().is_none().then_some(&patch.base),
        }
    }
    /// Whether `self` and `other` hold one buffer between them, neither with a
    /// copy of its own ([`Pixels::pair`], [`Pixels::share`]).
    pub fn shares_buffer(&self, other: &Self) -> bool {
        matches!((self.held(), other.held()), (Some(a), Some(b)) if Arc::ptr_eq(a, b))
    }
    /// The indices of the pixels this image keeps as differing from the buffer
    /// it holds with another image ([`Pixels::pair`]), ascending; none for
    /// every other image.
    pub fn differing(&self) -> impl Iterator<Item = usize> + '_ {
        let patch = match &self.0 {
            Repr::Patch(patch) => Some(patch),
            _ => None,
        };
        patch
            .into_iter()
            .flat_map(|patch| patch.differing(0..patch.base.len()).map(|(i, _)| i))
    }
}

impl Default for Pixels {
    fn default() -> Self {
        Self(Repr::Own(Vec::new()))
    }
}
impl From<Vec<[f32; 3]>> for Pixels {
    fn from(pixels: Vec<[f32; 3]>) -> Self {
        Self(Repr::Own(pixels))
    }
}
impl FromIterator<[f32; 3]> for Pixels {
    fn from_iter<I: IntoIterator<Item = [f32; 3]>>(pixels: I) -> Self {
        Vec::from_iter(pixels).into()
    }
}
impl FromParallelIterator<[f32; 3]> for Pixels {
    fn from_par_iter<I: IntoParallelIterator<Item = [f32; 3]>>(pixels: I) -> Self {
        Vec::from_par_iter(pixels).into()
    }
}

impl std::ops::Deref for Pixels {
    type Target = [[f32; 3]];
    fn deref(&self) -> &[[f32; 3]] {
        match &self.0 {
            Repr::Own(pixels) => pixels,
            Repr::Shared(pixels) => pixels,
            Repr::Patch(patch) => patch.whole(),
        }
    }
}
impl std::ops::DerefMut for Pixels {
    /// A buffer held with other images is left to them: this image takes a
    /// copy of its own first.
    fn deref_mut(&mut self) -> &mut [[f32; 3]] {
        if !matches!(self.0, Repr::Own(_)) {
            self.0 = Repr::Own(std::mem::take(self).into_vec());
        }
        match &mut self.0 {
            Repr::Own(pixels) => pixels,
            Repr::Shared(_) | Repr::Patch(_) => unreachable!("made its own above"),
        }
    }
}
/// One pixel is read where it is kept, so reading a decode pixel by pixel
/// never copies it out.
impl std::ops::Index<usize> for Pixels {
    type Output = [f32; 3];
    fn index(&self, i: usize) -> &[f32; 3] {
        match &self.0 {
            Repr::Own(pixels) => &pixels[i],
            Repr::Shared(pixels) => &pixels[i],
            Repr::Patch(patch) => patch.at(i),
        }
    }
}
impl std::ops::IndexMut<usize> for Pixels {
    fn index_mut(&mut self, i: usize) -> &mut [f32; 3] {
        &mut (**self)[i]
    }
}
/// A range is a slice of the whole image, as it was before one pixel had an
/// index of its own.
macro_rules! index_by_range {
    ($($range:ty),*) => {$(
        impl std::ops::Index<$range> for Pixels {
            type Output = [[f32; 3]];
            fn index(&self, range: $range) -> &[[f32; 3]] {
                &(**self)[range]
            }
        }
        impl std::ops::IndexMut<$range> for Pixels {
            fn index_mut(&mut self, range: $range) -> &mut [[f32; 3]] {
                &mut (**self)[range]
            }
        }
    )*};
}
index_by_range!(
    std::ops::Range<usize>,
    std::ops::RangeFrom<usize>,
    std::ops::RangeTo<usize>,
    std::ops::RangeInclusive<usize>,
    std::ops::RangeToInclusive<usize>,
    std::ops::RangeFull
);

impl IntoIterator for Pixels {
    type Item = [f32; 3];
    type IntoIter = std::vec::IntoIter<[f32; 3]>;
    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}
impl<'a> IntoIterator for &'a Pixels {
    type Item = &'a [f32; 3];
    type IntoIter = std::slice::Iter<'a, [f32; 3]>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl<'a> IntoIterator for &'a mut Pixels {
    type Item = &'a mut [f32; 3];
    type IntoIter = std::slice::IterMut<'a, [f32; 3]>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// Compared and printed without keeping a copy of an image that has none.
impl PartialEq for Pixels {
    fn eq(&self, other: &Self) -> bool {
        self.range(0..self.len()) == other.range(0..other.len())
    }
}
impl PartialEq<Vec<[f32; 3]>> for Pixels {
    fn eq(&self, other: &Vec<[f32; 3]>) -> bool {
        *self.range(0..self.len()) == **other
    }
}
impl PartialEq<[[f32; 3]]> for Pixels {
    fn eq(&self, other: &[[f32; 3]]) -> bool {
        *self.range(0..self.len()) == *other
    }
}
impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.range(0..self.len()).fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` pixels, each its index in every channel.
    fn ramp(n: usize) -> Vec<[f32; 3]> {
        (0..n).map(|i| [i as f32; 3]).collect()
    }

    /// A pair made of `first` and `second`, which differ wherever they differ.
    fn pair(first: &[[f32; 3]], second: &[[f32; 3]]) -> (Pixels, Pixels) {
        let changes = (0..first.len())
            .filter(|i| first[*i] != second[*i])
            .map(|i| (i, first[i]));
        Pixels::pair(second.to_vec(), changes).unwrap()
    }

    /// Both images of a pair read as they were made, one pixel, a range or the
    /// whole image at a time, wherever the pixels that differ fall in the words
    /// of the mask (the first and last pixel, both sides of a word's edge, a
    /// length that is and is not a whole number of words).
    #[test]
    fn a_pair_reads_as_its_two_images() {
        for n in [1, 63, 64, 65, 128, 200] {
            let second = ramp(n);
            for changed in [
                vec![0],
                vec![n - 1],
                vec![63, 64, 65, 127, 128],
                (0..n).collect(),
            ] {
                let mut first = second.clone();
                for &i in changed.iter().filter(|i| **i < n) {
                    first[i] = [-1. - i as f32; 3];
                }
                let (a, b) = pair(&first, &second);
                assert!(a.shares_buffer(&b), "{n} {changed:?}");
                assert_eq!((a.len(), b.len()), (n, n));
                for i in 0..n {
                    assert_eq!(a[i], first[i], "{n} {changed:?} pixel {i}");
                    assert_eq!(b[i], second[i]);
                }
                for start in [0, 1, 63, 64, 65, n / 2, n] {
                    for end in [start, start + 1, start + 64, n] {
                        if start <= end && end <= n {
                            assert_eq!(*a.range(start..end), first[start..end], "{start}..{end}");
                            assert_eq!(*b.range(start..end), second[start..end]);
                        }
                    }
                }
                let expected: Vec<usize> = (0..n).filter(|i| first[*i] != second[*i]).collect();
                assert_eq!(a.differing().collect::<Vec<_>>(), expected);
                assert_eq!(b.differing().count(), 0);
                // None of that copied the first image out; reading it as a slice does,
                // and gives the same pixels.
                assert!(a.shares_buffer(&b));
                assert_eq!(a, first);
                assert!(a.shares_buffer(&b));
                assert_eq!(&a[..], &first[..]);
                assert_eq!(a.shares_buffer(&b), expected.is_empty());
                assert_eq!(a[n - 1], first[n - 1]);
                assert_eq!(*a.range(0..n), first[..]);
                assert_eq!(a.into_vec(), first);
                assert_eq!(b.into_vec(), second);
            }
        }
    }

    /// Images that differ nowhere are one buffer, and a range without a pixel
    /// that differs is borrowed from it.
    #[test]
    fn equal_images_and_untouched_ranges_cost_no_copy() {
        let (a, b) = pair(&ramp(100), &ramp(100));
        assert!(a.shares_buffer(&b));
        assert_eq!(a.as_ptr(), b.as_ptr());
        assert!(a.shares_buffer(&b), "a shared buffer is read where it is");
        let mut first = ramp(200);
        first[150] = [9.; 3];
        let (a, b) = pair(&first, &ramp(200));
        assert!(matches!(a.range(0..150), Cow::Borrowed(_)));
        assert!(matches!(a.range(151..200), Cow::Borrowed(_)));
        assert!(matches!(a.range(100..160), Cow::Owned(_)));
        assert_eq!(a.range(0..150).as_ptr(), b.as_ptr());
    }

    /// Changes out of order, repeated or outside the buffer make no pair.
    #[test]
    fn changes_must_ascend_within_the_buffer() {
        let p = [1.; 3];
        assert!(Pixels::pair(ramp(4), [(2, p), (1, p)]).is_none());
        assert!(Pixels::pair(ramp(4), [(1, p), (1, p)]).is_none());
        assert!(Pixels::pair(ramp(4), [(4, p)]).is_none());
        assert!(Pixels::pair(ramp(4), [(0, p), (3, p)]).is_some());
        assert!(Pixels::pair(Vec::new(), []).is_some());
    }

    /// A mask of another length than the buffer, one that marks a pixel past the
    /// buffer's end, or one that marks more or fewer pixels than there are
    /// values makes no pair; a mask and its values make the pair their changes make.
    #[test]
    fn a_mask_must_fit_its_buffer_and_its_values() {
        let p = [1.; 3];
        assert!(Pixels::pair_masked(ramp(70), vec![1], vec![p]).is_none());
        assert!(Pixels::pair_masked(ramp(70), vec![1, 0, 0], vec![p]).is_none());
        assert!(Pixels::pair_masked(ramp(70), vec![0, 1 << 6], vec![p]).is_none());
        assert!(Pixels::pair_masked(ramp(70), vec![1, 1 << 5], vec![p]).is_none());
        assert!(Pixels::pair_masked(ramp(64), vec![1 << 63], vec![p, p]).is_none());
        assert!(Pixels::pair_masked(Vec::new(), vec![], vec![p]).is_none());
        assert!(Pixels::pair_masked(Vec::new(), vec![], vec![]).is_some());
        let (a, b) =
            Pixels::pair_masked(ramp(70), vec![1 << 63, 1 << 5], vec![p, [2.; 3]]).unwrap();
        let (c, d) = Pixels::pair(ramp(70), [(63, p), (69, [2.; 3])]).unwrap();
        assert_eq!((&a, &b), (&c, &d));
        assert_eq!(
            (a[63], a[69], a[64], b[63]),
            (p, [2.; 3], [64.; 3], [63.; 3])
        );
    }

    /// Writing to an image that holds its buffer with others gives it a buffer
    /// of its own and leaves theirs as it was; a copy of an image that owns its
    /// buffer is a buffer of its own, as it always was.
    #[test]
    fn a_write_never_reaches_another_image() {
        let mut first = ramp(70);
        first[5] = [-5.; 3];
        let (mut a, b) = pair(&first, &ramp(70));
        let also_a = a.clone();
        a[6] = [-6.; 3];
        assert!(!a.shares_buffer(&b) && also_a.shares_buffer(&b));
        assert_eq!(
            (a[5], a[6], also_a[6], b[6]),
            ([-5.; 3], [-6.; 3], [6.; 3], [6.; 3])
        );

        let mut shared: Pixels = ramp(8).into();
        shared.share();
        let mut copy = shared.clone();
        assert!(copy.shares_buffer(&shared));
        copy.fill([0.; 3]);
        assert!(!copy.shares_buffer(&shared));
        assert_eq!(shared, ramp(8));

        let own: Pixels = ramp(8).into();
        let copy = own.clone();
        assert!(!copy.shares_buffer(&own));
        assert_ne!(copy.as_ptr(), own.as_ptr());
    }
}
