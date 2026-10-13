//! `debug filter`: a table of the aircraft passing a rectangle around a
//! point, below an altitude.

use std::time::Instant;

use anyhow::Result;
use ratatui::Frame;
use rs1090::decode::TimedMessage;

use crate::commands::tui::{self, Column, View, age_style, opt};
use crate::domain::filter::{Filter, FilterParams, format_track};
use crate::source::stream::SourceArgs;

#[derive(clap::Args, Debug)]
pub struct FilterArgs {
    #[command(flatten)]
    pub source: SourceArgs,

    #[command(flatten)]
    pub params: FilterParams,
}

const COLUMNS: [Column; 11] = [
    ("ICAO", 6, false),
    ("CALLSIGN", 8, false),
    ("TYPE", 4, false),
    ("REG", 7, false),
    ("ALT ft", 7, true),
    ("DIST km", 7, true),
    ("MIN km", 6, true),
    ("TRACK", 5, true),
    ("GS kt", 6, true),
    ("FIXES", 5, true),
    ("AGE", 4, true),
];

/// The aircraft in the rectangle as a table on screen.
struct Passing(Filter);

impl View for Passing {
    fn update(&mut self, msg: &TimedMessage, now: Instant) -> bool {
        self.0.update(msg, now)
    }

    fn expire(&mut self, now: Instant) -> bool {
        self.0.expire(now)
    }

    fn draw(&self, frame: &mut Frame, scroll: &mut usize, status: &str, now: Instant) {
        let filter = &self.0;
        // By address, so that a row keeps its place while others come and go.
        let mut flights = filter.flights();
        flights.sort_by_key(|flight| flight.icao);

        let (lat, lon) = filter.center();
        let header = format!(
            "adsb-flipdot filter | {status} | {lat:.4},{lon:.4} {} rectangle @ {} <= {} ft | {} inside | {} passes | {} frames | {}",
            filter.dimensions(),
            format_track(filter.area().view_direction_deg),
            filter.max_altitude_ft(),
            flights.len(),
            filter.total_passes(),
            filter.frames(),
            filter.db_status(),
        );
        let rows = flights
            .into_iter()
            .map(|flight| {
                let age = now.duration_since(flight.last_fix);
                let cells = vec![
                    format!("{:06x}", flight.icao),
                    flight.callsign.unwrap_or_default(),
                    flight.info.typecode.unwrap_or_default(),
                    flight.info.registration.unwrap_or_default(),
                    opt(flight.altitude_ft, |v| v.to_string()),
                    format!("{:.1}", flight.distance_km),
                    format!("{:.1}", flight.min_distance_km),
                    opt(flight.track_deg, format_track),
                    opt(flight.groundspeed_kt, |v| format!("{v:.0}")),
                    flight.fixes.to_string(),
                    format!("{}s", age.as_secs()),
                ];
                (cells, age_style(age, filter.pass_timeout()))
            })
            .collect();
        tui::draw_table(
            frame,
            scroll,
            header,
            &COLUMNS,
            rows,
            "no aircraft in the rectangle",
        );
    }

    fn summary(&self) -> String {
        self.0.summary()
    }
}

pub async fn run(args: FilterArgs) -> Result<()> {
    let mut passing = Passing(args.params.build());
    tui::run(&args.source, &mut passing).await
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::commands::tui::tests::screen;
    use crate::domain::filter::Area;
    use crate::domain::filter::tests::overflown;

    #[test]
    fn draw_lists_passing_aircraft_with_track() {
        let start = Instant::now();
        let passing = Passing(overflown(start));

        let lines = screen(&passing, &mut 0, start + Duration::from_secs(4), 150, 8);
        assert!(
            lines[0].contains("53.7271,9.9066 10 x 10 km rectangle @ 360° <= 5000 ft"),
            "{}",
            lines[0]
        );
        assert!(lines[0].contains("1 inside | 1 passes"), "{}", lines[0]);
        assert!(
            lines[1]
                .starts_with("ICAO    CALLSIGN  TYPE  REG       ALT ft  DIST km  MIN km  TRACK"),
            "{}",
            lines[1]
        );
        let row = &lines[2];
        assert!(
            row.starts_with("3c0ac5  CFG5YW    A321  D-ATCA      1900"),
            "{row}"
        );
        assert!(lines[0].ends_with("6 aircraft known"), "{}", lines[0]);
        assert!(
            row.contains("    1.4     0.0   225°     151      5"),
            "{row}"
        );
        assert!(row.ends_with("  4s"), "{row}");
    }

    #[test]
    fn draw_empty_filter_shows_placeholder() {
        let area = Area {
            width_km: 10.0,
            length_km: 10.0,
            offset_percent: 50.0,
            view_direction_deg: 0.0,
        };
        let filter = Filter::new((53.7, 9.9), area, 5000, Duration::from_secs(30));
        let lines = screen(&Passing(filter), &mut 0, Instant::now(), 120, 8);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("no aircraft in the rectangle")),
            "{lines:#?}"
        );
    }
}
