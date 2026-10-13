//! `run`: show aircraft passing the point of interest on the flipdot display.
//!
//! Frames go through the same [`Filter`] as `debug filter`. The
//! [`Presenter`] decides which aircraft and which text is up. The display
//! is simulated in the terminal and, with `--gpio`, shown on the panels.

use std::time::Instant;

use anyhow::Result;

use crate::commands::flipdot::DisplayArgs;
use crate::commands::shutdown::Shutdown;
use crate::domain::filter::{Filter, FilterParams, Flight, format_track};
use crate::domain::presenter::Presenter;
use crate::domain::template::{Template, default_templates};
use crate::output::Note;
use crate::output::terminal::Stacking;
use crate::source::stream::{FrameStream, SourceArgs};

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

/// One line of plain text under the simulated display with the full details.
fn status_line(filter: &Filter, flights: &[Flight], shown: Option<usize>) -> String {
    match shown.and_then(|index| Some((index, flights.get(index)?))) {
        Some((index, flight)) => {
            let mut parts = vec![format!("{:06x}", flight.icao)];
            parts.extend(flight.callsign.clone());
            parts.extend(flight.info.registration.clone());
            parts.extend(
                flight
                    .info
                    .type_name
                    .clone()
                    .or(flight.info.typecode.clone()),
            );
            parts.extend(flight.altitude_ft.map(|v| format!("{v} ft")));
            parts.extend(flight.track_deg.map(format_track));
            parts.extend(flight.groundspeed_kt.map(|v| format!("{v:.0} kt")));
            parts.push(format!("{:.1} km", flight.distance_km));
            format!("{}  [{}/{}]", parts.join("  "), index + 1, flights.len())
        }
        None => format!(
            "no aircraft in the {} rectangle at or below {} ft  ({} passes, {} frames, {})",
            filter.dimensions(),
            filter.max_altitude_ft(),
            filter.total_passes(),
            filter.frames(),
            filter.db_status()
        ),
    }
}

pub async fn run(args: RunArgs) -> Result<()> {
    args.display.check()?;
    // Before the panels are opened: from here on a signal cannot end the
    // program in the middle of a pulse.
    let mut shutdown = Shutdown::listen()?;
    let templates = if args.templates.is_empty() {
        default_templates()
    } else {
        args.templates
    };
    // One tick moves a scrolling line by one dot.
    let mut presenter = Presenter::new(templates, args.page * u64::from(args.display.speed));
    let typesetting = args.display.typesetting();
    let mut filter = args.filter.build();
    let mut stream = FrameStream::open(&args.source).await?;
    eprintln!("adsb-flipdot: {}", stream.description());

    // The simulated display in the terminal, dots plus a status line, and
    // with `--gpio` the panels. Opened last: the threads of the source must
    // not inherit the real-time priority of the panels.
    let mut display = match args.display.open(Stacking::InPlace) {
        Ok(display) => display,
        Err(error) => {
            let _ = stream.close().await;
            return Err(error);
        }
    };
    let mut tick = tokio::time::interval(args.display.frame_time());
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut first = true;
    let mut failure = None;
    loop {
        tokio::select! {
            _ = shutdown.recv() => break,
            msg = stream.recv() => match msg {
                Some(msg) => {
                    filter.update(&msg, Instant::now());
                }
                None => break,
            },
            _ = tick.tick() => {
                filter.expire(Instant::now());
                let flights = filter.flights();
                let changed = presenter.step(&flights, &typesetting) || first;
                first = false;
                let status = status_line(&filter, &flights, presenter.shown(&flights));
                let note = Note {
                    caption: Some(&status),
                    new_text: changed,
                };
                if let Err(error) = display.show(&presenter.frame(&typesetting), note) {
                    failure = Some(error);
                    break;
                }
            }
        }
    }

    // Lowers every line of the panels.
    drop(display);
    let result = stream.close().await;
    eprintln!("adsb-flipdot: {}", filter.summary());
    failure.map_or(result, Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::filter::Area;
    use crate::domain::template::tests::{anonymous, easyjet};
    use std::time::Duration;

    #[test]
    fn status_line_describes_the_shown_aircraft() {
        let filter = Filter::new(
            (53.7, 9.9),
            Area {
                width_km: 4.0,
                length_km: 10.0,
                offset_percent: 50.0,
                view_direction_deg: 153.0,
            },
            5000,
            Duration::from_secs(30),
        );
        let flights = [easyjet(), anonymous(0xabc123)];
        assert_eq!(
            status_line(&filter, &flights, Some(0)),
            "405f12  EZY32GY  G-EZTB  AIRBUS A-320  1200 ft  153°  151 kt  1.5 km  [1/2]"
        );
        assert_eq!(
            status_line(&filter, &flights, Some(1)),
            "abc123  4.2 km  [2/2]"
        );
        assert_eq!(
            status_line(&filter, &[], None),
            "no aircraft in the 4 x 10 km rectangle at or below 5000 ft  (0 passes, 0 frames, aircraft db off)"
        );
    }
}
