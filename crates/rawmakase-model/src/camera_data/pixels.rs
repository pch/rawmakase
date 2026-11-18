//! The pixels of a [`CameraImage`](super::CameraImage).
use rayon::prelude::*;

/// A camera image's pixels, row by row: a slice of them wherever one is read
/// (`image.pixels[i]`, `image.pixels.iter()`, `&image.pixels`), made from a
/// `Vec` with `into()` or by collecting an iterator.
#[derive(Clone, Default)]
pub struct Pixels(Vec<[f32; 3]>);

impl Pixels {
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// The pixels as a `Vec` of their own.
    pub fn into_vec(self) -> Vec<[f32; 3]> {
        self.0
    }
}

impl From<Vec<[f32; 3]>> for Pixels {
    fn from(pixels: Vec<[f32; 3]>) -> Self {
        Self(pixels)
    }
}
impl FromIterator<[f32; 3]> for Pixels {
    fn from_iter<I: IntoIterator<Item = [f32; 3]>>(pixels: I) -> Self {
        Self(pixels.into_iter().collect())
    }
}
impl FromParallelIterator<[f32; 3]> for Pixels {
    fn from_par_iter<I: IntoParallelIterator<Item = [f32; 3]>>(pixels: I) -> Self {
        Self(pixels.into_par_iter().collect())
    }
}

impl std::ops::Deref for Pixels {
    type Target = [[f32; 3]];
    fn deref(&self) -> &[[f32; 3]] {
        &self.0
    }
}
impl std::ops::DerefMut for Pixels {
    fn deref_mut(&mut self) -> &mut [[f32; 3]] {
        &mut self.0
    }
}

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

impl PartialEq for Pixels {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}
impl PartialEq<Vec<[f32; 3]>> for Pixels {
    fn eq(&self, other: &Vec<[f32; 3]>) -> bool {
        **self == **other
    }
}
impl PartialEq<[[f32; 3]]> for Pixels {
    fn eq(&self, other: &[[f32; 3]]) -> bool {
        **self == *other
    }
}
impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}
