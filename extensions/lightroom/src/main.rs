//! The bundled Lightroom extension's program, which the app starts with its
//! channel attached.
fn main() -> std::process::ExitCode {
    match rawmakase_lightroom::extension(rawmakase_lightroom::Roots::here()).run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rawmakase-lightroom: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
