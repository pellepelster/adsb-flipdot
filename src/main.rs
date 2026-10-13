//! adsb-flipdot: receive Mode S / ADS-B frames from an RTL2832U dongle and
//! show the aircraft passing overhead on a flipdot display.
//!
//! The code is in layers. `cli` parses the options. `domain` is the
//! business code: which aircraft pass, what text they become, how it
//! scrolls and which dots have to flip. `output` shows the dots, in the
//! terminal and on the panels. `source` brings in the frames and the
//! aircraft database. `commands` holds the loops that wire these together.

mod cli;
mod commands;
mod domain;
mod output;
mod source;

use std::process::ExitCode;

use anyhow::Result;

use cli::{Command, DebugCommand};
use commands::{dump, filter, flipdot, live, pi, record, run};
use domain::template;

#[tokio::main]
async fn main() -> ExitCode {
    // Errors are printed here and not returned: returning them makes the
    // runtime print a backtrace whenever RUST_BACKTRACE is set.
    match run(cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(command: Command) -> Result<()> {
    match command {
        Command::Run(args) => run::run(args).await,
        Command::TemplateCodes => {
            print!("{}", template::help());
            Ok(())
        }
        Command::Debug { command } => match command {
            DebugCommand::Dump(args) => dump::run(args).await,
            DebugCommand::Record(args) => record::run(args).await,
            DebugCommand::Live(args) => live::run(args).await,
            DebugCommand::Filter(args) => filter::run(args).await,
            DebugCommand::Flipdot(args) => flipdot::run(args),
            DebugCommand::Pi(args) => pi::run(args).await,
        },
    }
}
