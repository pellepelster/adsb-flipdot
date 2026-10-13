//! What every aircraft in range broadcasts, merged from all its messages.
//!
//! Used by `debug live`, which lists it, and by the aircraft filter, which
//! needs the resolved positions.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use rs1090::decode::cpr::{AircraftState, decode_position};
use rs1090::decode::{DF, ICAO, Message, TimedMessage};
use serde_json::Value;

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

/// The aircraft in range. Pure state, no I/O, so it can be unit tested.
pub struct Tracker {
    pub aircraft: BTreeMap<u32, Aircraft>,
    /// rs1090's own per-aircraft state, needed to pair CPR position frames.
    cpr: BTreeMap<ICAO, AircraftState>,
    /// An aircraft is dropped after this long without a message.
    pub expire: Duration,
    pub frames: u64,
}

impl Tracker {
    pub fn new(expire: Duration) -> Self {
        Self {
            aircraft: BTreeMap::new(),
            cpr: BTreeMap::new(),
            expire,
            frames: 0,
        }
    }

    /// Merge one frame. Returns the address of the aircraft it told
    /// something about, if any.
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
        // formats may update a known aircraft but never add one.
        let verified = matches!(df, "11" | "17" | "18");
        let icao = match json.get("icao24").and_then(Value::as_str) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => decoded.crc,
        };
        if !verified && !self.aircraft.contains_key(&icao) {
            return None;
        }

        let aircraft = self.aircraft.entry(icao).or_default();
        aircraft.messages += 1;
        aircraft.last_seen = Some(now);
        if let Some(rssi) = msg.metadata.first().and_then(|m| m.rssi) {
            aircraft.rssi_db = Some(rssi);
        }
        if let Some(v) = json.get("callsign").and_then(Value::as_str) {
            aircraft.callsign = Some(v.trim_end_matches([' ', '_']).to_string());
        }
        if let Some(v) = json.get("squawk").and_then(Value::as_str) {
            aircraft.squawk = Some(v.to_string());
        }
        if let Some(v) = json.get("altitude").and_then(Value::as_i64) {
            aircraft.altitude_ft = Some(v);
        }
        if let Some(v) = json.get("groundspeed").and_then(Value::as_f64) {
            aircraft.groundspeed_kt = Some(v);
        }
        if let Some(v) = json.get("track").and_then(Value::as_f64) {
            aircraft.track_deg = Some(v);
        }
        if let Some(v) = json.get("vertical_rate").and_then(Value::as_i64) {
            aircraft.vertical_rate_fpm = Some(v);
        }
        if let (Some(lat), Some(lon)) = (
            json.get("latitude").and_then(Value::as_f64),
            json.get("longitude").and_then(Value::as_f64),
        ) {
            aircraft.latitude = Some(lat);
            aircraft.longitude = Some(lon);
            aircraft.position_time = Some(msg.timestamp);
        }
        Some(icao)
    }

    /// Drop aircraft not heard from within the expiry time. Returns true if
    /// any was dropped.
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.aircraft.len();
        let expire = self.expire;
        self.aircraft.retain(|_, a| {
            a.last_seen
                .is_some_and(|seen| now.duration_since(seen) <= expire)
        });
        let aircraft = &self.aircraft;
        self.cpr.retain(|icao, _| aircraft.contains_key(&icao.0));
        self.aircraft.len() != before
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::source::tests::timed;

    // Reference frames from "The 1090 Megahertz Riddle" (Junzi Sun).
    pub(crate) const IDENT: &str = "8d4840d6202cc371c32ce0576098"; // 4840d6, KLM1023
    pub(crate) const VELOCITY: &str = "8d485020994409940838175b284f"; // 485020, 159 kt
    pub(crate) const POS_EVEN: &str = "8d40621d58c382d690c8ac2863a7"; // 40621d
    pub(crate) const POS_ODD: &str = "8d40621d58c386435cc412692ad6";
    const SECS_30: Duration = Duration::from_secs(30);

    #[test]
    fn identification_adds_an_aircraft_with_callsign() {
        let mut t = Tracker::new(SECS_30);
        assert_eq!(t.update(&timed(IDENT, 1.0), Instant::now()), Some(0x4840d6));
        let a = &t.aircraft[&0x4840d6];
        assert_eq!(a.callsign.as_deref(), Some("KLM1023"));
        assert_eq!(a.messages, 1);
    }

    #[test]
    fn velocity_fills_speed_track_and_vertical_rate() {
        let mut t = Tracker::new(SECS_30);
        t.update(&timed(VELOCITY, 1.0), Instant::now());
        let a = &t.aircraft[&0x485020];
        assert!((a.groundspeed_kt.unwrap() - 159.2).abs() < 0.1);
        assert!((a.track_deg.unwrap() - 182.88).abs() < 0.01);
        assert_eq!(a.vertical_rate_fpm, Some(-832));
    }

    #[test]
    fn position_pair_resolves_latitude_and_longitude() {
        let mut t = Tracker::new(SECS_30);
        let now = Instant::now();
        t.update(&timed(POS_ODD, 1.0), now);
        assert_eq!(
            t.aircraft[&0x40621d].latitude, None,
            "one frame is not enough"
        );
        assert_eq!(t.aircraft[&0x40621d].position_time, None);
        t.update(&timed(POS_EVEN, 2.0), now);
        let a = &t.aircraft[&0x40621d];
        assert!((a.latitude.unwrap() - 52.2572).abs() < 0.001, "{a:?}");
        assert!((a.longitude.unwrap() - 3.9194).abs() < 0.001, "{a:?}");
        assert_eq!(a.position_time, Some(2.0));
        assert_eq!(a.altitude_ft, Some(38000));
        assert_eq!(a.messages, 2);
    }

    #[test]
    fn unverified_format_does_not_add_an_aircraft() {
        // DF4 altitude reply: the address comes from the checksum, so it
        // cannot be trusted on its own.
        let mut t = Tracker::new(SECS_30);
        assert_eq!(
            t.update(&timed("200004bb11d2c1", 1.0), Instant::now()),
            None
        );
        assert!(t.aircraft.is_empty());
    }

    #[test]
    fn unverified_format_updates_a_known_aircraft() {
        let mut t = Tracker::new(SECS_30);
        let now = Instant::now();
        // The aircraft is known from an earlier verified message.
        t.aircraft.insert(0x503d48, Aircraft::default());
        assert_eq!(t.update(&timed("200004bb11d2c1", 2.0), now), Some(0x503d48));
        let a = &t.aircraft[&0x503d48];
        assert_eq!(a.altitude_ft, Some(6875));
        assert_eq!(a.messages, 1);
    }

    #[test]
    fn aircraft_expire_after_the_timeout() {
        let mut t = Tracker::new(SECS_30);
        let start = Instant::now();
        t.update(&timed(IDENT, 1.0), start);
        t.update(&timed(VELOCITY, 1.0), start + Duration::from_secs(20));

        assert!(!t.expire(start + Duration::from_secs(30)), "30 s is kept");
        assert_eq!(t.aircraft.len(), 2);

        assert!(t.expire(start + Duration::from_secs(31)));
        assert!(!t.aircraft.contains_key(&0x4840d6), "stale one removed");
        assert!(t.aircraft.contains_key(&0x485020), "fresh one kept");

        assert!(t.expire(start + Duration::from_secs(60)));
        assert!(t.aircraft.is_empty());
        assert!(t.cpr.is_empty(), "decoder state is pruned too");
    }
}
