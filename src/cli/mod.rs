//! The command line: every option of every command, and how the options
//! turn into the plain values the other layers work with.
//!
//! Nothing outside this module parses options, and the layers below the
//! commands do not know that options exist.

mod display;
mod filter;
pub mod parse;
mod source;

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use display::{BoardArgs, DisplayArgs};
use filter::FilterParams;
use source::SourceArgs;

use crate::domain::template::Template;

#[derive(Parser, Debug)]
#[command(name = "adsb-flipdot", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Show aircraft passing the point of interest on the flipdot display.
    Run(RunArgs),
    /// List the codes that can be used in `run --template`.
    TemplateCodes,
    /// Diagnostic tools.
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum DebugCommand {
    /// Print every received Mode S / ADS-B frame to the console.
    Dump(DumpArgs),
    /// Save every received frame to a JSON lines file, for replay with
    /// --device-record-file.
    Record(RecordArgs),
    /// Show a continuously updated table with one row per aircraft.
    Live(LiveArgs),
    /// Show aircraft passing a rectangle around a point, below an altitude.
    Filter(FilterArgs),
    /// Preview text as dots, the way the flipdot panel will show it.
    Flipdot(FlipdotArgs),
    /// Switch the signals of the adapter board on the Raspberry Pi by hand.
    Pi(PiArgs),
}

/// Read the command line. Prints the help or what is wrong with the
/// options and ends the program if there is no command to run.
pub fn parse() -> Command {
    Cli::parse().command
}

#[derive(clap::Args, Debug)]
pub struct RunArgs {
    #[command(flatten)]
    pub source: SourceArgs,

    #[command(flatten)]
    pub filter: FilterParams,

    #[command(flatten)]
    pub display: DisplayArgs,

    /// Text to show per aircraft, with placeholders such as
    /// `{{ callsign }} {{ aircraft code }} {{ track }}`. `\n` starts a second
    /// line. Repeat the option to alternate between several pages. Run
    /// `adsb-flipdot template-codes` for all codes.
    #[arg(long = "template", value_name = "TEMPLATE", value_parser = Template::parse)]
    pub templates: Vec<Template>,

    /// Show each page for this many seconds. A scrolling page always
    /// finishes its pass, so it may stay longer.
    #[arg(long, value_name = "SECONDS", default_value_t = 3,
          value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub page: u64,
}

#[derive(clap::Args, Debug)]
pub struct DumpArgs {
    #[command(flatten)]
    pub source: SourceArgs,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    pub format: Format,
}

/// How `debug dump` prints a frame.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// One `*<hex>;` line per frame, as dump1090 emits on port 30002.
    Raw,
    /// Human readable decode of every frame.
    Text,
    /// One JSON object per line with frame, timestamp, RSSI and decode.
    Json,
}

#[derive(clap::Args, Debug)]
pub struct RecordArgs {
    /// File to write, one JSON object per line.
    #[arg(value_name = "FILE")]
    pub output: PathBuf,

    /// Replace the file if it already exists.
    #[arg(long, conflicts_with = "append")]
    pub overwrite: bool,

    /// Add to the file if it already exists.
    #[arg(long)]
    pub append: bool,

    /// Stop after this many seconds. Without it, record until Ctrl-C.
    #[arg(long, value_name = "SECONDS")]
    pub duration: Option<u64>,

    #[command(flatten)]
    pub source: SourceArgs,
}

#[derive(clap::Args, Debug)]
pub struct LiveArgs {
    #[command(flatten)]
    pub source: SourceArgs,

    /// Remove an aircraft after this many seconds without a message.
    #[arg(long, value_name = "SECONDS", default_value_t = 30)]
    pub expire: u64,
}

#[derive(clap::Args, Debug)]
pub struct FilterArgs {
    #[command(flatten)]
    pub source: SourceArgs,

    #[command(flatten)]
    pub params: FilterParams,
}

#[derive(clap::Args, Debug)]
pub struct FlipdotArgs {
    /// Text to show. Each argument is one line; `\n` inside an argument also
    /// starts a new line. Without any text, lines are read from stdin and
    /// each one is rendered as its own frame.
    pub text: Vec<String>,

    #[command(flatten)]
    pub display: DisplayArgs,
}

#[derive(clap::Args, Debug)]
pub struct PiArgs {
    #[command(flatten)]
    pub board: BoardArgs,

    /// Touch no pin, to try the interface away from the Pi.
    #[arg(long)]
    pub dry_run: bool,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::domain::scene::{Font, Scroll};

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
        assert_eq!(args.format, Format::Raw);
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
        assert_eq!(args.display.font, Font::F4x6);
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
        assert_eq!(args.board.chip, std::path::PathBuf::from("/dev/gpiochip0"));
        assert_eq!((args.board.pulse_us, args.dry_run), (190, false));

        let Command::Debug {
            command: DebugCommand::Pi(args),
        } = parse(&["--dry-run", "--pulse-us", "1000"]).unwrap().command
        else {
            panic!("expected debug pi");
        };
        assert_eq!((args.board.pulse_us, args.dry_run), (1000, true));

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
        assert_eq!(args.display.scroll, Scroll::Always);

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
