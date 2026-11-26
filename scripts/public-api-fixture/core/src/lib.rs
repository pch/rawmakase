//! The shapes scripts/public-api.py must render.
#![allow(dead_code)]
pub use rawmakase_fixture_sub::Shown;

pub type Result<T, E = Error> = std::result::Result<T, E>;
pub struct Error;

#[non_exhaustive]
pub enum Rich {
    Unit,
    Tuple(u32, String),
    Struct { a: i32, b: bool },
    #[non_exhaustive]
    Ext { c: u8 },
}
pub mod variants {
    pub use crate::Rich::*;
}
pub use Rich::Unit as Nothing;

pub struct Opaque(u32);
pub struct HalfOpen(pub u32, String);
pub struct Marker;

pub struct Boxed<T>(pub T);
impl Boxed<u32> {
    pub fn reset(&mut self) {
        self.0 = 0;
    }
}

pub struct Letters(Vec<char>);
impl std::ops::Deref for Letters {
    type Target = [char];
    fn deref(&self) -> &[char] {
        &self.0
    }
}
pub trait Named {
    fn name(&self) -> String;
}
impl Named for String {
    fn name(&self) -> String {
        self.clone()
    }
}
pub type Callback = Box<dyn for<'a> Fn(&'a str) + Send>;
pub type Raw = unsafe extern "C" fn(u8) -> u8;

pub trait Run<T>: Send {
    type Output;
    const LIMIT: usize;
    fn run(&self, x: T) -> Self::Output;
    fn ready(&self) -> bool {
        true
    }
}

#[non_exhaustive]
pub struct Holder<T = u32> {
    pub t: T,
}
impl<T: Clone> Holder<T> {
    pub fn with<U>(&self, _u: U) -> T
    where
        U: Into<String>,
    {
        self.t.clone()
    }
    pub async fn wait(&self) {}
}

pub fn takes(shown: &Shown) -> Option<&Shown> {
    Some(shown)
}
