//! The bundled MIDI extension's program, which the app starts with its
//! channel attached.
fn main() -> std::process::ExitCode {
    match rawmakase_midi::extension().run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rawmakase-midi: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
