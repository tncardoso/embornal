use clap::Parser;
use embornal::cli::{self, Cli};
use std::io::Write;

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    match cli::run(cli, &mut out) {
        Ok(()) => {
            let _ = out.flush();
            std::process::ExitCode::SUCCESS
        }
        Err(err) => {
            let _ = out.flush();
            eprintln!("embornal: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}
