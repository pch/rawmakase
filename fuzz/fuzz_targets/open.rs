//! The header facts of any bytes: an error, never a panic.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let _ = rawmakase_raw::open(bytes);
});
