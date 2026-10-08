use std::process::ExitCode;

use herdr_inbox::cli::{self, Command};

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("herdr-inbox: {message}");
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Help => {
            print!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("herdr-inbox {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Run(options) => match herdr_inbox::runtime::run(options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("herdr-inbox: {err:#}");
                ExitCode::FAILURE
            }
        },
    }
}
