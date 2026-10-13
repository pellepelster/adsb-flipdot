//! adsb-flipdot: receive Mode S / ADS-B frames from an RTL2832U dongle.

mod aircraft_db;
mod board;
mod display;
mod dump;
mod filter;
mod flipdot;
mod live;
mod panel;
mod pi;
mod record;
mod run;
mod scene;
mod source;
mod template;
mod tui;

use std::process::ExitCode;

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
    /// Show aircraft passing the point of interest on the flipdot display.
    Run(run::RunArgs),
    /// List the codes that can be used in `run --template`.
    TemplateCodes,
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
    /// Save every received frame to a JSON lines file, for replay with
    /// --device-record-file.
    Record(record::RecordArgs),
    /// Show a continuously updated table with one row per aircraft.
    Live(live::LiveArgs),
    /// Show aircraft passing a rectangle around a point, below an altitude.
    Filter(filter::FilterArgs),
    /// Preview text as dots, the way the flipdot panel will show it.
    Flipdot(flipdot::FlipdotArgs),
    /// Switch the signals of the adapter board on the Raspberry Pi by hand.
    Pi(pi::PiArgs),
}

#[tokio::main]
async fn main() -> ExitCode {
    // Errors are printed here and not returned: returning them makes the
    // runtime print a backtrace whenever RUST_BACKTRACE is set.
    match run(Cli::parse().command).await {
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
pub(crate) mod tests {
    use super::*;

    /// Parse `extra` as the options of one `clap::Args` struct on its own,
    /// the way `Cli` would see them.
    pub(crate) fn try_parse_args<A: clap::Args>(extra: &[&str]) -> Result<A, clap::Error> {
        let matches = A::augment_args(clap::Command::new("test"))
            .try_get_matches_from(["test"].iter().chain(extra))?;
        A::from_arg_matches(&matches)
    }

    /// Like [`try_parse_args`], for options that are known to be valid.
    pub(crate) fn parse_args<A: clap::Args>(extra: &[&str]) -> A {
        try_parse_args(extra).unwrap_or_else(|e| panic!("{extra:?}: {e}"))
    }

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
    fn debug_flipdot_parses() {
        let cli = Cli::try_parse_from([
            "adsb-flipdot",
            "debug",
            "flipdot",
            "SAS51B",
            "FL390",
            "--font",
            "4x6",
        ])
        .unwrap();
        let Command::Debug {
            command: DebugCommand::Flipdot(args),
        } = cli.command
        else {
            panic!("expected debug flipdot");
        };
        assert_eq!(args.text, ["SAS51B", "FL390"]);
        assert_eq!(args.display.font, scene::Font::F4x6);
        let display = &args.display;
        assert_eq!((display.width, display.height, display.panels), (28, 14, 2));
        assert_eq!(display.display_size(), (56, 14));
    }

    #[test]
    fn debug_pi_parses() {
        let parse = |extra: &[&str]| {
            Cli::try_parse_from(["adsb-flipdot", "debug", "pi"].iter().chain(extra))
        };
        let Command::Debug {
            command: DebugCommand::Pi(args),
        } = parse(&[]).unwrap().command
        else {
            panic!("expected debug pi");
        };
        assert_eq!(args.chip, std::path::PathBuf::from("/dev/gpiochip0"));
        assert_eq!((args.pulse_us, args.dry_run), (190, false));

        let Command::Debug {
            command: DebugCommand::Pi(args),
        } = parse(&["--dry-run", "--pulse-us", "1000"]).unwrap().command
        else {
            panic!("expected debug pi");
        };
        assert_eq!((args.pulse_us, args.dry_run), (1000, true));

        assert!(parse(&["--pulse-us", "0"]).is_err());
        assert!(parse(&["--pulse-us", "25001"]).is_err(), "above 25 ms");
    }

    #[test]
    fn run_parses_filter_and_display_options() {
        let cli = Cli::try_parse_from([
            "adsb-flipdot",
            "run",
            "--area-width",
            "8",
            "--area-length",
            "12",
            "--area-offset",
            "75",
            "--view-direction",
            "90",
            "--max-altitude",
            "3000",
            "--panels",
            "3",
            "--page",
            "4",
            "--cache-dir",
            "/tmp/x",
            "--no-aircraft-db",
        ])
        .unwrap();
        let Command::Run(args) = cli.command else {
            panic!("expected run");
        };
        assert_eq!(
            (args.filter.area_width, args.filter.area_length),
            (8.0, 12.0)
        );
        assert_eq!(args.filter.area_offset, 75.0);
        assert_eq!(args.filter.max_altitude, 3000);
        assert_eq!(args.filter.view_direction, 90.0);
        assert_eq!((args.filter.lat, args.filter.lon), (53.7271256, 9.9065838));
        assert_eq!(args.display.display_size(), (84, 14));
        assert_eq!(args.page, 4);
        assert_eq!(
            args.filter.db.cache_dir.as_deref(),
            Some(std::path::Path::new("/tmp/x"))
        );
        assert!(args.filter.db.no_aircraft_db);
    }

    #[test]
    fn run_accepts_several_templates_and_rejects_bad_ones() {
        let cli = Cli::try_parse_from([
            "adsb-flipdot",
            "run",
            "--template",
            "{{ callsign }} {{ aircraft code }} {{ track }}",
            "--template",
            "{{ name }}\\n{{ altitude }}",
            "--scroll",
            "always",
        ])
        .unwrap();
        let Command::Run(args) = cli.command else {
            panic!("expected run");
        };
        assert_eq!(args.templates.len(), 2);
        assert_eq!(args.display.scroll, scene::Scroll::Always);

        let error = Cli::try_parse_from(["adsb-flipdot", "run", "--template", "{{ wingspan }}"])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unknown template code `wingspan`"),
            "{error}"
        );
    }

    #[test]
    fn template_codes_command_exists() {
        let cli = Cli::try_parse_from(["adsb-flipdot", "template-codes"]).unwrap();
        assert!(matches!(cli.command, Command::TemplateCodes));
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
