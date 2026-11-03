//! Named slots of a flat `f32` parameter array that a shader reads, declared once so the
//! Rust side and the shader's generated offset constants cannot drift apart.

/// Declares each slot as a `Range` that starts where the previous one ends, the
/// array's length as `HEADER`, and every slot by name as `ALL`.
macro_rules! slots {
    ($($name:ident: $len:expr),* $(,)?) => {
        $crate::develop::slots::slots!(@at 0; $($name: $len,)*);
        /// Every slot, in order, by the name the shader gives it.
        pub(crate) const ALL: &[(&str, ::std::ops::Range<usize>)] =
            &[$((stringify!($name), $name)),*];
    };
    (@at $at:expr;) => {
        /// The number of parameters the slots take.
        pub(crate) const HEADER: usize = $at;
    };
    (@at $at:expr; $name:ident: $len:expr, $($rest:tt)*) => {
        pub(crate) const $name: ::std::ops::Range<usize> = $at..$at + $len;
        $crate::develop::slots::slots!(@at $name.end; $($rest)*);
    };
}
pub(crate) use slots;
