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

use display::{BoardArgs, DisplayArgs, PanelsArgs};
use filter::FilterParams;
use source::SourceArgs;

use crate::domain::pattern::Pattern;
use crate::domain::scene::Font;

#[derive(Parser, Debug)]
#[command(name = "adsb-flipdot", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Show aircraft passing the point of interest on the flipdot display.
    Live(LiveArgs),
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
    All(AllArgs),
    /// Show aircraft passing a rectangle around a point, below an altitude.
    Filter(FilterArgs),
    /// Preview text as dots, the way the flipdot panel will show it.
    Flipdot(FlipdotArgs),
    /// Switch the signals of the adapter board on the Raspberry Pi by hand.
    Pi(PiArgs),
    /// Turn every dot of the panels to its dark side.
    Clear(ClearArgs),
    /// Show test patterns one after another, until stopped.
    Pattern(PatternArgs),
}

/// Read the command line. Prints the help or what is wrong with the
/// options and ends the program if there is no command to run.
pub fn parse() -> Command {
    Cli::parse().command
}

#[derive(clap::Args, Debug)]
pub struct LiveArgs {
    #[command(flatten)]
    pub source: SourceArgs,

    #[command(flatten)]
    pub filter: FilterParams,

    #[command(flatten)]
    pub display: DisplayArgs,

    /// Drive the flipdot panels through the adapter board on the Pi's GPIO
    /// pins, next to the preview in the terminal.
    #[arg(long)]
    pub gpio: bool,

    /// Show each page for this many seconds. A scrolling page always
    /// finishes its pass, so it may stay longer.
    #[arg(long, value_name = "SECONDS", default_value_t = 3,
          value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub page_wait: u64,
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
pub struct AllArgs {
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

    /// Font, named by the size of one character cell in dots.
    #[arg(long, value_enum, default_value_t = Font::F5x7)]
    pub font: Font,

    #[command(flatten)]
    pub display: DisplayArgs,

    /// Only show the dots in the terminal; without it the panels are driven
    /// through the adapter board as well.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(clap::Args, Debug)]
pub struct ClearArgs {
    #[command(flatten)]
    pub panels: PanelsArgs,
}

#[derive(clap::Args, Debug)]
pub struct PatternArgs {
    /// The patterns to show, in this order. Without it all of them.
    #[arg(long = "pattern", value_enum, value_name = "PATTERN")]
    pub patterns: Vec<Pattern>,

    /// Show each pattern for this many seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 3,
          value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub hold: u64,

    /// Go through the patterns once and end, instead of until stopped.
    #[arg(long)]
    pub once: bool,

    /// Only show the patterns in the terminal; without it the panels are
    /// driven through the adapter board as well.
    #[arg(long)]
    pub dry_run: bool,

    #[command(flatten)]
    pub panels: PanelsArgs,
}

#[derive(clap::Args, Debug)]
pub struct PiArgs {
    #[command(flatten)]
    pub board: BoardArgs,

    /// Touch no pin, to try the interface away from the Pi.
    #[arg(long)]
    pub dry_run: bool,

    /// Flip every dot of the panel on COL-SEL0 in turn until stopped: 1/1
    /// black, 1/2 yellow and so on, the two colours swapped on every pass.
    #[arg(long)]
    pub continuous: bool,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::domain::scene::Scroll;

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
        assert_eq!(args.source.gain, None);
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
        assert_eq!(args.font, Font::F4x6);
        let display = &args.display;
        let panels = &display.panels;
        assert_eq!((panels.width, panels.height, panels.count), (28, 16, 1));
        assert_eq!(display.display_size(), (28, 16));
        assert!(!args.dry_run, "drives the panels unless told not to");

        let cli =
            Cli::try_parse_from(["adsb-flipdot", "debug", "flipdot", "--dry-run", "x"]).unwrap();
        let Command::Debug {
            command: DebugCommand::Flipdot(args),
        } = cli.command
        else {
            panic!("expected debug flipdot");
        };
        assert!(args.dry_run);
        assert!(
            Cli::try_parse_from(["adsb-flipdot", "debug", "flipdot", "--gpio", "x"]).is_err(),
            "--gpio belongs to run"
        );
    }

    #[test]
    fn debug_pattern_parses() {
        let parse = |extra: &[&str]| {
            let cli = Cli::try_parse_from(["adsb-flipdot", "debug", "pattern"].iter().chain(extra))
                .unwrap();
            let Command::Debug {
                command: DebugCommand::Pattern(args),
            } = cli.command
            else {
                panic!("expected debug pattern");
            };
            args
        };
        let args = parse(&[]);
        assert!(args.patterns.is_empty(), "all of them");
        assert_eq!((args.hold, args.once, args.dry_run), (3, false, false));

        let args = parse(&[
            "--pattern",
            "top-left",
            "--pattern",
            "checkerboard-inverted",
            "--hold",
            "1",
            "--once",
            "--dry-run",
            "--upside-down",
        ]);
        assert_eq!(
            args.patterns,
            [Pattern::TopLeft, Pattern::CheckerboardInverted]
        );
        assert_eq!((args.hold, args.once, args.dry_run), (1, true, true));
        assert!(args.panels.panel.upside_down);
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
        assert!(!args.continuous);

        let Command::Debug {
            command: DebugCommand::Pi(args),
        } = parse(&["--continuous"]).unwrap().command
        else {
            panic!("expected debug pi");
        };
        assert!(args.continuous);

        assert!(parse(&["--pulse-us", "0"]).is_err());
        assert!(parse(&["--pulse-us", "25001"]).is_err(), "above 25 ms");
    }

    #[test]
    fn run_parses_filter_and_display_options() {
        let cli = Cli::try_parse_from([
            "adsb-flipdot",
            "live",
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
            "--page-wait",
            "4",
            "--cache-dir",
            "/tmp/x",
            "--no-aircraft-db",
            "--gpio",
        ])
        .unwrap();
        let Command::Live(args) = cli.command else {
            panic!("expected run");
        };
        assert!(args.gpio);
        assert_eq!(
            (args.filter.area_width, args.filter.area_length),
            (8.0, 12.0)
        );
        assert_eq!(args.filter.area_offset, 75.0);
        assert_eq!(args.filter.max_altitude, 3000);
        assert_eq!(args.filter.view_direction, 90.0);
        assert_eq!((args.filter.lat, args.filter.lon), (53.7271256, 9.9065838));
        assert_eq!(args.display.display_size(), (84, 16));
        assert_eq!(args.page_wait, 4);
        assert_eq!(
            args.filter.db.cache_dir.as_deref(),
            Some(std::path::Path::new("/tmp/x"))
        );
        assert!(args.filter.db.no_aircraft_db);
    }

    #[test]
    fn live_shows_the_standard_pages_for_three_seconds() {
        let cli = Cli::try_parse_from(["adsb-flipdot", "live", "--scroll", "always"]).unwrap();
        let Command::Live(args) = cli.command else {
            panic!("expected live");
        };
        assert_eq!(args.display.scroll, Scroll::Always);
        assert_eq!(
            args.page_wait, 3,
            "3 seconds per page unless told otherwise"
        );

        let parse = |args: &[&str]| Cli::try_parse_from(args.iter().copied());
        assert!(parse(&["adsb-flipdot", "live", "--template", "{{ name }}"]).is_err());
        assert!(parse(&["adsb-flipdot", "debug", "templates"]).is_err());
        assert!(parse(&["adsb-flipdot", "live", "--page-wait", "0"]).is_err());
    }

    #[test]
    fn debug_all_parses() {
        let cli = Cli::try_parse_from(["adsb-flipdot", "debug", "all", "--expire", "10"]).unwrap();
        let Command::Debug {
            command: DebugCommand::All(args),
        } = cli.command
        else {
            panic!("expected debug all");
        };
        assert_eq!(args.expire, 10);
    }
}
