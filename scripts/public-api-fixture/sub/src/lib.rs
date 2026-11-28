//! A crate under the fixture library.
mod hidden {
    /// Defined in a private module and re-exported at the root.
    pub struct Shown {
        pub value: u8,
        kept: u8,
    }
    impl Shown {
        pub fn kept(&self) -> u8 {
            self.kept
        }
    }
}
pub use hidden::Shown;
