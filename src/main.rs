//! adsb-flipdot: receive Mode S / ADS-B frames from an RTL2832U dongle.

mod dump;
mod live;
mod source;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "adsb-flipdot", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Diagnostic tools.
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
}

#[derive(Subcommand, Debug)]
enum DebugCommand {
    /// Print every received Mode S / ADS-B frame to the console.
    Dump(dump::DumpArgs),
    /// Show a continuously updated table with one row per aircraft.
    Live(live::LiveArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Debug { command } => match command {
            DebugCommand::Dump(args) => dump::run(args).await,
            DebugCommand::Live(args) => live::run(args).await,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn debug_dump_parses() {
        let cli =
            Cli::try_parse_from(["adsb-flipdot", "debug", "dump", "--format", "raw"]).unwrap();
        let Command::Debug {
            command: DebugCommand::Dump(args),
        } = cli.command
        else {
            panic!("expected debug dump");
        };
        assert_eq!(args.format, dump::Format::Raw);
        assert_eq!(args.source.gain, 496);
    }

    #[test]
    fn debug_live_parses() {
        let cli = Cli::try_parse_from(["adsb-flipdot", "debug", "live", "--expire", "10"]).unwrap();
        let Command::Debug {
            command: DebugCommand::Live(args),
        } = cli.command
        else {
            panic!("expected debug live");
        };
        assert_eq!(args.expire, 10);
    }
}
