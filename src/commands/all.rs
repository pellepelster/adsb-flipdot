//! `debug all`: a continuously updated table with one row per aircraft.

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::Frame;
use rs1090::decode::TimedMessage;

use crate::cli::AllArgs;
use crate::commands::tui::{self, Column, View, age_style, opt};
use crate::domain::tracker::Tracker;

const COLUMNS: [Column; 12] = [
    ("ICAO", 6, false),
    ("CALLSIGN", 8, false),
    ("SQUAWK", 6, true),
    ("ALT ft", 7, true),
    ("GS kt", 6, true),
    ("TRK", 5, true),
    ("VS fpm", 7, true),
    ("LAT", 9, true),
    ("LON", 10, true),
    ("RSSI", 6, true),
    ("MSGS", 6, true),
    ("AGE", 4, true),
];

impl View for Tracker {
    fn update(&mut self, msg: &TimedMessage, now: Instant) -> bool {
        Tracker::update(self, msg, now).is_some()
    }

    fn expire(&mut self, now: Instant) -> bool {
        Tracker::expire(self, now)
    }

    fn draw(&self, frame: &mut Frame, scroll: &mut usize, status: &str, now: Instant) {
        let header = format!(
            "adsb-flipdot all | {status} | {} aircraft | {} frames | expire {} s",
            self.aircraft.len(),
            self.frames,
            self.expire.as_secs()
        );
        let rows = self
            .aircraft
            .iter()
            .map(|(icao, a)| {
                let age = a
                    .last_seen
                    .map(|seen| now.duration_since(seen))
                    .unwrap_or_default();
                let cells = vec![
                    format!("{icao:06x}"),
                    a.callsign.clone().unwrap_or_default(),
                    a.squawk.clone().unwrap_or_default(),
                    opt(a.altitude_ft, |v| v.to_string()),
                    opt(a.groundspeed_kt, |v| format!("{v:.0}")),
                    opt(a.track_deg, |v| format!("{v:.0}°")),
                    opt(a.vertical_rate_fpm, |v| format!("{v:+}")),
                    opt(a.latitude, |v| format!("{v:.4}")),
                    opt(a.longitude, |v| format!("{v:.4}")),
                    opt(a.rssi_db, |v| format!("{v:.1}")),
                    a.messages.to_string(),
                    format!("{}s", age.as_secs()),
                ];
                (cells, age_style(age, self.expire))
            })
            .collect();
        tui::draw_table(
            frame,
            scroll,
            header,
            &COLUMNS,
            rows,
            "waiting for aircraft",
        );
    }

    fn summary(&self) -> String {
        format!(
            "{} frames received, {} aircraft in view at exit",
            self.frames,
            self.aircraft.len()
        )
    }
}

pub async fn run(args: AllArgs) -> Result<()> {
    let mut tracker = Tracker::new(Duration::from_secs(args.expire));
    tui::run(&args.source.source(), &mut tracker).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tui::tests::screen;
    use crate::domain::tracker::tests::timed;
    use crate::domain::tracker::tests::{IDENT, VELOCITY};

    const SECS_30: Duration = Duration::from_secs(30);

    /// A tracker that has heard from two aircraft at `now`.
    fn two_aircraft(now: Instant) -> Tracker {
        let mut t = Tracker::new(SECS_30);
        t.update(&timed(IDENT, 1.0), now);
        t.update(&timed(VELOCITY, 1.0), now);
        t
    }

    #[test]
    fn draw_shows_header_columns_and_aircraft_row() {
        let start = Instant::now();
        let t = two_aircraft(start);

        let lines = screen(&t, &mut 0, start + Duration::from_secs(7), 110, 10);
        assert!(
            lines[0].contains("2 aircraft | 2 frames | expire 30 s"),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].starts_with("ICAO    CALLSIGN  SQUAWK"),
            "{}",
            lines[1]
        );
        assert!(lines[2].starts_with("4840d6  KLM1023"), "{}", lines[2]);
        assert!(lines[2].ends_with("  7s"), "{}", lines[2]);
        // 485020: 159 kt, track 183, descending 832 ft/min.
        assert!(lines[3].starts_with("485020"), "{}", lines[3]);
        assert!(lines[3].contains("159"), "{}", lines[3]);
        assert!(lines[3].contains("183°"), "{}", lines[3]);
        assert!(lines[3].contains("-832"), "{}", lines[3]);
    }

    #[test]
    fn draw_without_aircraft_shows_placeholder() {
        let t = Tracker::new(SECS_30);
        let lines = screen(&t, &mut 0, Instant::now(), 110, 8);
        assert!(
            lines.iter().any(|l| l.contains("waiting for aircraft")),
            "{lines:#?}"
        );
    }

    #[test]
    fn draw_scrolls_and_clamps_the_offset() {
        let now = Instant::now();
        let t = two_aircraft(now);

        // Scrolled by one: the first aircraft is off screen.
        let mut scroll = 1;
        let lines = screen(&t, &mut scroll, now, 110, 10);
        assert!(lines[2].starts_with("485020"), "{}", lines[2]);

        // Far past the end: clamped to the last row.
        let mut scroll = 99;
        screen(&t, &mut scroll, now, 110, 10);
        assert_eq!(scroll, 1);
    }
}
