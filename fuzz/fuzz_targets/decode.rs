//! The sensor data of any bytes `open` accepts.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::atomic::AtomicBool;

fuzz_target!(|bytes: &[u8]| {
    if let Ok(info) = rawmakase_raw::open(bytes) {
        let _ = rawmakase_raw::decode(&info, bytes, &AtomicBool::new(false));
    }
});
