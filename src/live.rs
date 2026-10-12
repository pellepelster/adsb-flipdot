//! `debug live`: a continuously updated table with one row per aircraft.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::Frame;
use rs1090::decode::cpr::{AircraftState, decode_position};
use rs1090::decode::{DF, ICAO, Message, TimedMessage};
use serde_json::Value;

use crate::source::SourceArgs;
use crate::tui::{self, Column, View, age_style, opt};

#[derive(clap::Args, Debug)]
pub struct LiveArgs {
    #[command(flatten)]
    pub source: SourceArgs,

    /// Remove an aircraft after this many seconds without a message.
    #[arg(long, value_name = "SECONDS", default_value_t = 30)]
    pub expire: u64,
}

/// Everything known about one aircraft, merged from all its messages.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Aircraft {
    pub callsign: Option<String>,
    pub squawk: Option<String>,
    pub altitude_ft: Option<i64>,
    pub groundspeed_kt: Option<f64>,
    pub track_deg: Option<f64>,
    pub vertical_rate_fpm: Option<i64>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// Receive time, in seconds, of the frame that gave the last position.
    pub position_time: Option<f64>,
    pub rssi_db: Option<f32>,
    pub messages: u64,
    pub last_seen: Option<Instant>,
}

/// The aircraft table. Pure state, no I/O, so it can be unit tested.
pub struct Table {
    pub rows: BTreeMap<u32, Aircraft>,
    /// rs1090's own per-aircraft state, needed to pair CPR position frames.
    cpr: BTreeMap<ICAO, AircraftState>,
    expire: Duration,
    pub frames: u64,
}

impl Table {
    pub fn new(expire: Duration) -> Self {
        Self {
            rows: BTreeMap::new(),
            cpr: BTreeMap::new(),
            expire,
            frames: 0,
        }
    }

    /// Merge one frame into the table. Returns the address of the aircraft
    /// whose row changed, if any.
    pub fn update(&mut self, msg: &TimedMessage, now: Instant) -> Option<u32> {
        self.frames += 1;
        let mut decoded = Message::try_from(msg.frame.as_slice()).ok()?;

        // Resolve latitude/longitude from pairs of CPR encoded position frames.
        if let DF::ExtendedSquitterADSB(adsb) = &mut decoded.df {
            let icao = adsb.icao24;
            decode_position(
                &mut adsb.message,
                msg.timestamp,
                &icao,
                &mut self.cpr,
                &mut None,
                &None,
            );
        }

        let json = serde_json::to_value(&decoded).ok()?;
        let df = json.get("df").and_then(Value::as_str).unwrap_or("");

        // Only all-call replies and extended squitters carry a checksum that
        // proves the address. For every other format the address is derived
        // from the checksum itself, so noise produces random aircraft. Those
        // formats may update a known aircraft but never create a row.
        let verified = matches!(df, "11" | "17" | "18");
        let icao = match json.get("icao24").and_then(Value::as_str) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => decoded.crc,
        };
        if !verified && !self.rows.contains_key(&icao) {
            return None;
        }

        let row = self.rows.entry(icao).or_default();
        row.messages += 1;
        row.last_seen = Some(now);
        if let Some(rssi) = msg.metadata.first().and_then(|m| m.rssi) {
            row.rssi_db = Some(rssi);
        }
        if let Some(v) = json.get("callsign").and_then(Value::as_str) {
            row.callsign = Some(v.trim_end_matches([' ', '_']).to_string());
        }
        if let Some(v) = json.get("squawk").and_then(Value::as_str) {
            row.squawk = Some(v.to_string());
        }
        if let Some(v) = json.get("altitude").and_then(Value::as_i64) {
            row.altitude_ft = Some(v);
        }
        if let Some(v) = json.get("groundspeed").and_then(Value::as_f64) {
            row.groundspeed_kt = Some(v);
        }
        if let Some(v) = json.get("track").and_then(Value::as_f64) {
            row.track_deg = Some(v);
        }
        if let Some(v) = json.get("vertical_rate").and_then(Value::as_i64) {
            row.vertical_rate_fpm = Some(v);
        }
        if let (Some(lat), Some(lon)) = (
            json.get("latitude").and_then(Value::as_f64),
            json.get("longitude").and_then(Value::as_f64),
        ) {
            row.latitude = Some(lat);
            row.longitude = Some(lon);
            row.position_time = Some(msg.timestamp);
        }
        Some(icao)
    }

    /// Drop aircraft not heard from within the expiry time. Returns true if
    /// any row was removed.
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.rows.len();
        let expire = self.expire;
        self.rows.retain(|_, a| {
            a.last_seen
                .is_some_and(|seen| now.duration_since(seen) <= expire)
        });
        let rows = &self.rows;
        self.cpr.retain(|icao, _| rows.contains_key(&icao.0));
        self.rows.len() != before
    }
}

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

impl View for Table {
    fn update(&mut self, msg: &TimedMessage, now: Instant) -> bool {
        Table::update(self, msg, now).is_some()
    }

    fn expire(&mut self, now: Instant) -> bool {
        Table::expire(self, now)
    }

    fn draw(&self, frame: &mut Frame, scroll: &mut usize, status: &str, now: Instant) {
        let header = format!(
            "adsb-flipdot live | {status} | {} aircraft | {} frames | expire {} s",
            self.rows.len(),
            self.frames,
            self.expire.as_secs()
        );
        let rows = self
            .rows
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
            self.rows.len()
        )
    }
}

pub async fn run(args: LiveArgs) -> Result<()> {
    let mut table = Table::new(Duration::from_secs(args.expire));
    tui::run(&args.source, &mut table).await
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::source::tests::timed;
    use crate::tui::tests::screen;

    // Reference frames from "The 1090 Megahertz Riddle" (Junzi Sun).
    pub(crate) const IDENT: &str = "8d4840d6202cc371c32ce0576098"; // 4840d6, KLM1023
    pub(crate) const VELOCITY: &str = "8d485020994409940838175b284f"; // 485020, 159 kt
    pub(crate) const POS_EVEN: &str = "8d40621d58c382d690c8ac2863a7"; // 40621d
    pub(crate) const POS_ODD: &str = "8d40621d58c386435cc412692ad6";
    const SECS_30: Duration = Duration::from_secs(30);

    #[test]
    fn identification_creates_row_with_callsign() {
        let mut t = Table::new(SECS_30);
        assert_eq!(t.update(&timed(IDENT, 1.0), Instant::now()), Some(0x4840d6));
        let a = &t.rows[&0x4840d6];
        assert_eq!(a.callsign.as_deref(), Some("KLM1023"));
        assert_eq!(a.messages, 1);
    }

    #[test]
    fn velocity_fills_speed_track_and_vertical_rate() {
        let mut t = Table::new(SECS_30);
        t.update(&timed(VELOCITY, 1.0), Instant::now());
        let a = &t.rows[&0x485020];
        assert!((a.groundspeed_kt.unwrap() - 159.2).abs() < 0.1);
        assert!((a.track_deg.unwrap() - 182.88).abs() < 0.01);
        assert_eq!(a.vertical_rate_fpm, Some(-832));
    }

    #[test]
    fn position_pair_resolves_latitude_and_longitude() {
        let mut t = Table::new(SECS_30);
        let now = Instant::now();
        t.update(&timed(POS_ODD, 1.0), now);
        assert_eq!(t.rows[&0x40621d].latitude, None, "one frame is not enough");
        assert_eq!(t.rows[&0x40621d].position_time, None);
        t.update(&timed(POS_EVEN, 2.0), now);
        let a = &t.rows[&0x40621d];
        assert!((a.latitude.unwrap() - 52.2572).abs() < 0.001, "{a:?}");
        assert!((a.longitude.unwrap() - 3.9194).abs() < 0.001, "{a:?}");
        assert_eq!(a.position_time, Some(2.0));
        assert_eq!(a.altitude_ft, Some(38000));
        assert_eq!(a.messages, 2);
    }

    #[test]
    fn unverified_format_does_not_create_a_row() {
        // DF4 altitude reply: the address comes from the checksum, so it
        // cannot be trusted on its own.
        let mut t = Table::new(SECS_30);
        assert_eq!(
            t.update(&timed("200004bb11d2c1", 1.0), Instant::now()),
            None
        );
        assert!(t.rows.is_empty());
    }

    #[test]
    fn unverified_format_updates_a_known_aircraft() {
        let mut t = Table::new(SECS_30);
        let now = Instant::now();
        // The aircraft is known from an earlier verified message.
        t.rows.insert(0x503d48, Aircraft::default());
        assert_eq!(t.update(&timed("200004bb11d2c1", 2.0), now), Some(0x503d48));
        let a = &t.rows[&0x503d48];
        assert_eq!(a.altitude_ft, Some(6875));
        assert_eq!(a.messages, 1);
    }

    #[test]
    fn rows_expire_after_the_timeout() {
        let mut t = Table::new(SECS_30);
        let start = Instant::now();
        t.update(&timed(IDENT, 1.0), start);
        t.update(&timed(VELOCITY, 1.0), start + Duration::from_secs(20));

        assert!(!t.expire(start + Duration::from_secs(30)), "30 s is kept");
        assert_eq!(t.rows.len(), 2);

        assert!(t.expire(start + Duration::from_secs(31)));
        assert!(!t.rows.contains_key(&0x4840d6), "stale row removed");
        assert!(t.rows.contains_key(&0x485020), "fresh row kept");

        assert!(t.expire(start + Duration::from_secs(60)));
        assert!(t.rows.is_empty());
        assert!(t.cpr.is_empty(), "decoder state is pruned too");
    }

    #[test]
    fn draw_shows_header_columns_and_aircraft_row() {
        let mut t = Table::new(SECS_30);
        let start = Instant::now();
        t.update(&timed(IDENT, 1.0), start);
        t.update(&timed(VELOCITY, 1.0), start);

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
    fn draw_empty_table_shows_placeholder() {
        let t = Table::new(SECS_30);
        let lines = screen(&t, &mut 0, Instant::now(), 110, 8);
        assert!(
            lines.iter().any(|l| l.contains("waiting for aircraft")),
            "{lines:#?}"
        );
    }

    #[test]
    fn draw_scrolls_and_clamps_the_offset() {
        let mut t = Table::new(SECS_30);
        let now = Instant::now();
        t.update(&timed(IDENT, 1.0), now);
        t.update(&timed(VELOCITY, 1.0), now);

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
