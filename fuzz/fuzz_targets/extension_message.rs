//! What an extension's program, or a package, hands the app: any bytes read
//! as protocol lines and as a manifest give an error, never a panic, a hang
//! or an unbounded allocation.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rawmakase_extension_protocol::framing::LineReader;
use rawmakase_extension_protocol::manifest::Manifest;
use rawmakase_extension_protocol::method::{AppEvent, AppRequest, ProgramEvent, ProgramRequest};
use rawmakase_extension_protocol::{Message, controls};

fuzz_target!(|bytes: &[u8]| {
    let mut lines = LineReader::new(bytes);
    while let Ok(Some(line)) = lines.next_message() {
        let Ok(message) = line else { continue };
        match message {
            Message::Req { m, p, .. } => {
                let _ = ProgramRequest::parse(&m, p.clone());
                let _ = AppRequest::parse(&m, p);
            }
            Message::Event { e, p } => {
                if let Ok(ProgramEvent::PanelUpdate(update)) = ProgramEvent::parse(&e, p.clone()) {
                    let _ = controls::validate(&update.controls);
                }
                let _ = AppEvent::parse(&e, p);
            }
            _ => {}
        }
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        let _ = Manifest::parse(text);
    }
});
