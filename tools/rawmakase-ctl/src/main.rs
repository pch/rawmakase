#![forbid(unsafe_code)]
use clap::Parser;
fn main() -> std::process::ExitCode {
    match rawmakase_ctl::run(rawmakase_ctl::Cli::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rawmakase-ctl: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
