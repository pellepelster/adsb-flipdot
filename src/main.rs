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

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// What the files of a layer must not mention: the libraries of other
    /// layers, and the layers above or beside it.
    const RULES: [(&str, &[&str]); 4] = [
        (
            "domain",
            &[
                "clap",
                "ratatui",
                "crossterm",
                "gpiocdev",
                "libc",
                "tokio",
                "ureq",
                "std::fs",
                "std::net",
                "println!",
                "crate::cli",
                "crate::commands",
                "crate::output",
                "crate::source",
            ],
        ),
        (
            "output",
            &[
                "clap",
                "ratatui",
                "crossterm",
                "crate::cli",
                "crate::commands",
                "crate::source",
            ],
        ),
        (
            "source",
            &[
                "clap",
                "ratatui",
                "crossterm",
                "crate::cli",
                "crate::commands",
                "crate::output",
            ],
        ),
        ("cli", &["ratatui", "crossterm", "crate::commands"]),
    ];

    /// Every line in the files of `layer` that mentions one of `forbidden`.
    fn violations(layer: &str, forbidden: &[&str]) -> Vec<String> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join(layer);
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            for (number, line) in text.lines().enumerate() {
                for word in forbidden {
                    if line.contains(word) {
                        let place = format!("{}:{}", path.display(), number + 1);
                        found.push(format!("{place} mentions `{word}`"));
                    }
                }
            }
        }
        found
    }

    #[test]
    fn the_layers_keep_to_themselves() {
        for (layer, forbidden) in RULES {
            let found = violations(layer, forbidden);
            assert!(
                found.is_empty(),
                "`{layer}` reaches outside its layer:\n{}",
                found.join("\n")
            );
        }
    }

    #[test]
    fn the_check_finds_what_it_looks_for() {
        // `cli` is where clap belongs, so looking for it there has to hit.
        assert!(!violations("cli", &["clap"]).is_empty());
        assert!(violations("domain", &["no such library"]).is_empty());
    }
}
