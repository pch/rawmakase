//! The embedded preview of any bytes `open` accepts, and the one a thumbnail
//! would be made from.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    if let Ok(info) = rawmakase_raw::open(bytes) {
        let _ = rawmakase_raw::preview(&info, bytes);
        let _ = rawmakase_raw::preview_reaching(&info, bytes, 256);
    }
});
