//! The sensor data of any bytes `open` accepts.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rawmakase_cancel::Cancel;

fuzz_target!(|bytes: &[u8]| {
    if let Ok(info) = rawmakase_raw::open(bytes) {
        let _ = rawmakase_raw::decode(&info, bytes, Cancel::NEVER);
    }
});
