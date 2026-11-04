//! Regression and diagnosis tools for the command line's `fingerprint`, `decode-dump`
//! and `half-check` commands: what opening and unpacking a file yields
//! ([`fingerprint`]), what each decode mode makes of it ([`decode_dump`]), and how far
//! a half-size draft is from the full decode ([`half_check`]). Nothing a photo is
//! opened, shown or exported with uses them.
pub mod decode_dump;
pub mod fingerprint;
pub mod half_check;
