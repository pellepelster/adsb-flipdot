//! Aircraft filter: which aircraft are currently inside a rectangle around a
//! point and below an altitude limit.
//!
//! Used by `debug filter`, which lists them in a table, and by `run`, which
//! shows them on the flipdot display.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::Frame;
use rs1090::decode::TimedMessage;

use crate::aircraft_db::{AircraftInfo, DbArgs, Lookup};
use crate::live::Table;
use crate::source::SourceArgs;
use crate::tui::{self, Column, View, age_style, opt};

/// Mean Earth radius in kilometres.
const EARTH_RADIUS_KM: f64 = 6371.0088;
/// How long the underlying aircraft tracker keeps a silent aircraft.
const TRACKER_EXPIRE: Duration = Duration::from_secs(60);

#[derive(clap::Args, Debug)]
pub struct FilterArgs {
    #[command(flatten)]
    pub source: SourceArgs,

    #[command(flatten)]
    pub params: FilterParams,
}

/// Which aircraft to select. Shared by `debug filter` and `run`.
#[derive(clap::Args, Debug)]
pub struct FilterParams {
    /// Latitude of the point of interest, in degrees.
    #[arg(long, default_value_t = 53.7271256, allow_negative_numbers = true,
          value_parser = parse_latitude)]
    pub lat: f64,

    /// Longitude of the point of interest, in degrees.
    #[arg(long, default_value_t = 9.9065838, allow_negative_numbers = true,
          value_parser = parse_longitude)]
    pub lon: f64,

    /// Width of the rectangle, across the view direction, in kilometres.
    #[arg(long, value_name = "KM", default_value_t = 4.0, value_parser = parse_positive)]
    pub area_width: f64,

    /// Length of the rectangle, along the view direction, in kilometres.
    #[arg(long, value_name = "KM", default_value_t = 6.0, value_parser = parse_positive)]
    pub area_length: f64,

    /// How much of the length lies ahead of the point, in percent. 50
    /// centres the rectangle on the point, 100 puts all of it ahead of you
    /// and 0 all of it behind you.
    #[arg(long, value_name = "PERCENT", default_value_t = 80.0, value_parser = parse_percent)]
    pub area_offset: f64,

    /// Compass direction you look in from the point, in degrees. Two edges
    /// of the rectangle run parallel to it.
    #[arg(long, value_name = "DEGREES", default_value_t = 153.0, value_parser = parse_direction)]
    pub view_direction: f64,

    /// Only aircraft at or below this altitude, in feet.
    #[arg(
        long,
        value_name = "FEET",
        default_value_t = 5000,
        allow_negative_numbers = true
    )]
    pub max_altitude: i64,

    /// Drop an aircraft after this many seconds without a position report,
    /// in case its signal is lost while inside the rectangle.
    #[arg(long, value_name = "SECONDS", default_value_t = 30)]
    pub expire: u64,

    #[command(flatten)]
    pub db: DbArgs,
}

impl FilterParams {
    pub fn build(&self) -> Filter {
        Filter::new(
            (self.lat, self.lon),
            Area {
                width_km: self.area_width,
                length_km: self.area_length,
                offset_percent: self.area_offset,
                view_direction_deg: self.view_direction,
            },
            self.max_altitude,
            Duration::from_secs(self.expire),
        )
        .with_lookup(self.db.start())
    }
}

fn parse_in_range(s: &str, limit: f64, what: &str) -> Result<f64, String> {
    let value: f64 = s.parse().map_err(|_| format!("`{s}` is not a number"))?;
    if value.is_finite() && value.abs() <= limit {
        Ok(value)
    } else {
        Err(format!("{what} must be between -{limit} and {limit}"))
    }
}

fn parse_latitude(s: &str) -> Result<f64, String> {
    parse_in_range(s, 90.0, "latitude")
}

fn parse_longitude(s: &str) -> Result<f64, String> {
    parse_in_range(s, 180.0, "longitude")
}

fn parse_direction(s: &str) -> Result<f64, String> {
    let value: f64 = s.parse().map_err(|_| format!("`{s}` is not a number"))?;
    if (0.0..=360.0).contains(&value) {
        Ok(value)
    } else {
        Err("direction must be between 0 and 360".to_string())
    }
}

fn parse_positive(s: &str) -> Result<f64, String> {
    let value: f64 = s.parse().map_err(|_| format!("`{s}` is not a number"))?;
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err("must be greater than 0".to_string())
    }
}

fn parse_percent(s: &str) -> Result<f64, String> {
    let number = s.strip_suffix('%').unwrap_or(s);
    let value: f64 = number
        .parse()
        .map_err(|_| format!("`{s}` is not a number"))?;
    if (0.0..=100.0).contains(&value) {
        Ok(value)
    } else {
        Err("must be between 0 and 100".to_string())
    }
}

/// The rectangle on the ground that aircraft have to be over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    /// Edge across the view direction.
    pub width_km: f64,
    /// Edge along the view direction.
    pub length_km: f64,
    /// Share of the length that lies ahead of the point.
    pub offset_percent: f64,
    /// Compass direction the length runs along.
    pub view_direction_deg: f64,
}

impl Area {
    /// Whether a point `along` the view direction and `across` it, both in
    /// kilometres from the point of interest, lies in the rectangle.
    fn contains(&self, along: f64, across: f64) -> bool {
        let ahead = self.length_km * self.offset_percent / 100.0;
        let behind = self.length_km - ahead;
        (-behind..=ahead).contains(&along) && across.abs() <= self.width_km / 2.0
    }
}

/// Great-circle distance between two points in kilometres (haversine).
pub fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * a.sqrt().asin()
}

/// Offset of a point from `center` in kilometres, as (east, north). Flat
/// approximation, good for the few kilometres around the centre.
fn offset_km(center: (f64, f64), lat: f64, lon: f64) -> (f64, f64) {
    let north = (lat - center.0).to_radians() * EARTH_RADIUS_KM;
    let dlon = (lon - center.1 + 180.0).rem_euclid(360.0) - 180.0;
    let east = dlon.to_radians() * EARTH_RADIUS_KM * center.0.to_radians().cos();
    (east, north)
}

/// A resolved position.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fix {
    /// Receive time in seconds.
    time: f64,
    lat: f64,
    lon: f64,
}

/// Aviation style track: three digits, north is 360 rather than 000.
pub fn format_track(course: f64) -> String {
    let rounded = course.round().rem_euclid(360.0) as u32;
    format!("{:03}°", if rounded == 0 { 360 } else { rounded })
}

/// One aircraft's passage through the rectangle, kept only while it is inside.
#[derive(Debug, Clone, PartialEq)]
struct Pass {
    /// Counts the passes; a lower number entered the rectangle earlier.
    entered: u64,
    /// Number of positions received inside the rectangle.
    fixes: u64,
    /// Closest approach to the point so far.
    min_distance_km: f64,
    /// Distance at the latest position.
    distance_km: f64,
    /// When the latest position arrived.
    last_fix: Instant,
}

/// An aircraft currently inside the rectangle, with what it broadcasts.
#[derive(Debug, Clone, PartialEq)]
pub struct Flight {
    pub icao: u32,
    pub callsign: Option<String>,
    pub altitude_ft: Option<i64>,
    pub track_deg: Option<f64>,
    pub groundspeed_kt: Option<f64>,
    pub distance_km: f64,
    /// Registration and type from the aircraft database, if known.
    pub info: AircraftInfo,
}

/// Filter state: which aircraft are inside the rectangle. Pure state, no I/O.
pub struct Filter {
    center: (f64, f64),
    area: Area,
    max_altitude_ft: i64,
    expire: Duration,
    /// Tracks every aircraft in range and resolves positions.
    tracker: Table,
    passes: BTreeMap<u32, Pass>,
    /// Time of the last position handled per aircraft, to spot new ones.
    handled: BTreeMap<u32, f64>,
    /// Aircraft that passed since start.
    total_passes: u64,
    /// Resolves registration and type from the ICAO address.
    lookup: Lookup,
}

impl Filter {
    pub fn new(center: (f64, f64), area: Area, max_altitude_ft: i64, expire: Duration) -> Self {
        Self {
            center,
            area,
            max_altitude_ft,
            expire,
            tracker: Table::new(TRACKER_EXPIRE),
            passes: BTreeMap::new(),
            handled: BTreeMap::new(),
            total_passes: 0,
            lookup: Lookup::disabled(),
        }
    }

    /// Resolve aircraft registration and type with `lookup`.
    pub fn with_lookup(mut self, lookup: Lookup) -> Self {
        self.lookup = lookup;
        self
    }

    /// State of the aircraft database, for status lines.
    pub fn db_status(&self) -> String {
        self.lookup.status()
    }

    /// Aircraft currently inside the rectangle, the one that entered first
    /// at the front.
    pub fn flights(&self) -> Vec<Flight> {
        let mut passes: Vec<_> = self.passes.iter().collect();
        passes.sort_by_key(|(_, pass)| pass.entered);
        passes
            .into_iter()
            .map(|(icao, pass)| {
                let aircraft = self.tracker.rows.get(icao);
                Flight {
                    icao: *icao,
                    callsign: aircraft.and_then(|a| a.callsign.clone()),
                    altitude_ft: aircraft.and_then(|a| a.altitude_ft),
                    track_deg: aircraft.and_then(|a| a.track_deg),
                    groundspeed_kt: aircraft.and_then(|a| a.groundspeed_kt),
                    distance_km: pass.distance_km,
                    info: self.lookup.get(*icao).unwrap_or_default(),
                }
            })
            .collect()
    }

    /// Frames received so far.
    pub fn frames(&self) -> u64 {
        self.tracker.frames
    }

    /// Aircraft that entered the rectangle since start.
    pub fn total_passes(&self) -> u64 {
        self.total_passes
    }

    /// Width and length of the rectangle, for status lines.
    pub fn dimensions(&self) -> String {
        format!("{} x {} km", self.area.width_km, self.area.length_km)
    }

    /// Whether a point lies in the rectangle, which has two edges parallel
    /// to the view direction.
    fn contains(&self, lat: f64, lon: f64) -> bool {
        let (east, north) = offset_km(self.center, lat, lon);
        let (sin, cos) = self.area.view_direction_deg.to_radians().sin_cos();
        let along = north * cos + east * sin;
        let across = east * cos - north * sin;
        self.area.contains(along, across)
    }

    pub fn max_altitude_ft(&self) -> i64 {
        self.max_altitude_ft
    }

    /// Record a resolved position. Returns true if the listing changed.
    ///
    /// The aircraft is listed for exactly as long as its positions qualify:
    /// the first position outside removes it at once.
    ///
    /// A position qualifies when it is inside the rectangle and the aircraft is
    /// at or below the altitude limit. An unknown altitude qualifies: only
    /// surface position reports come without one.
    fn add_position(
        &mut self,
        icao: u32,
        fix: Fix,
        altitude_ft: Option<i64>,
        now: Instant,
    ) -> bool {
        let distance = distance_km(self.center.0, self.center.1, fix.lat, fix.lon);
        let qualifies = self.contains(fix.lat, fix.lon)
            && altitude_ft.is_none_or(|alt| alt <= self.max_altitude_ft);

        if !qualifies {
            // Left the rectangle, or climbed above the limit.
            return self.passes.remove(&icao).is_some();
        }

        let pass = self.passes.entry(icao).or_insert_with(|| {
            self.total_passes += 1;
            Pass {
                entered: self.total_passes,
                fixes: 0,
                min_distance_km: distance,
                distance_km: distance,
                last_fix: now,
            }
        });
        pass.fixes += 1;
        pass.min_distance_km = pass.min_distance_km.min(distance);
        pass.distance_km = distance;
        pass.last_fix = now;
        true
    }
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

impl View for Filter {
    fn update(&mut self, msg: &TimedMessage, now: Instant) -> bool {
        let Some(icao) = self.tracker.update(msg, now) else {
            return false;
        };
        let aircraft = &self.tracker.rows[&icao];
        let (Some(lat), Some(lon), Some(time)) = (
            aircraft.latitude,
            aircraft.longitude,
            aircraft.position_time,
        ) else {
            return false;
        };
        let altitude = aircraft.altitude_ft;

        // Only act on a position that has not been handled yet. Other
        // messages still refresh callsign and altitude of a listed pass.
        if self.handled.get(&icao) == Some(&time) {
            return self.passes.contains_key(&icao);
        }
        self.handled.insert(icao, time);
        self.add_position(icao, Fix { time, lat, lon }, altitude, now)
    }

    fn expire(&mut self, now: Instant) -> bool {
        self.tracker.expire(now);
        let tracker = &self.tracker;
        self.handled
            .retain(|icao, _| tracker.rows.contains_key(icao));

        let before = self.passes.len();
        let expire = self.expire;
        self.passes
            .retain(|_, pass| now.duration_since(pass.last_fix) <= expire);
        self.passes.len() != before
    }

    fn draw(&self, frame: &mut Frame, scroll: &mut usize, status: &str, now: Instant) {
        let header = format!(
            "adsb-flipdot filter | {status} | {:.4},{:.4} {} rectangle @ {} <= {} ft | {} inside | {} passes | {} frames | {}",
            self.center.0,
            self.center.1,
            self.dimensions(),
            format_track(self.area.view_direction_deg),
            self.max_altitude_ft,
            self.passes.len(),
            self.total_passes,
            self.tracker.frames,
            self.lookup.status(),
        );
        let rows = self
            .passes
            .iter()
            .map(|(icao, pass)| {
                let aircraft = self.tracker.rows.get(icao);
                let info = self.lookup.get(*icao).unwrap_or_default();
                let age = now.duration_since(pass.last_fix);
                let cells = vec![
                    format!("{icao:06x}"),
                    opt(aircraft.and_then(|a| a.callsign.clone()), |v| v),
                    info.typecode.unwrap_or_default(),
                    info.registration.unwrap_or_default(),
                    opt(aircraft.and_then(|a| a.altitude_ft), |v| v.to_string()),
                    format!("{:.1}", pass.distance_km),
                    format!("{:.1}", pass.min_distance_km),
                    opt(aircraft.and_then(|a| a.track_deg), format_track),
                    opt(aircraft.and_then(|a| a.groundspeed_kt), |v| {
                        format!("{v:.0}")
                    }),
                    pass.fixes.to_string(),
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
            "no aircraft in the rectangle",
        );
    }

    fn summary(&self) -> String {
        format!(
            "{} frames received, {} aircraft entered the rectangle",
            self.tracker.frames, self.total_passes
        )
    }
}

pub async fn run(args: FilterArgs) -> Result<()> {
    let mut filter = args.params.build();
    tui::run(&args.source, &mut filter).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::tests::{IDENT, POS_EVEN, POS_ODD};
    use crate::source::tests::timed;
    use crate::tui::tests::screen;

    /// The default point of interest.
    const CENTER: (f64, f64) = (53.7271256, 9.9065838);
    const SECS_30: Duration = Duration::from_secs(30);
    const KM_PER_DEG: f64 = 111.195;

    /// A 10 km square centred on the point.
    fn square(view_direction_deg: f64) -> Area {
        Area {
            width_km: 10.0,
            length_km: 10.0,
            offset_percent: 50.0,
            view_direction_deg,
        }
    }

    fn filter() -> Filter {
        Filter::new(CENTER, square(0.0), 5000, SECS_30)
    }

    /// A point `east_km` and `north_km` away from the centre.
    fn offset(east_km: f64, north_km: f64) -> (f64, f64) {
        (
            CENTER.0 + north_km / KM_PER_DEG,
            CENTER.1 + east_km / (KM_PER_DEG * CENTER.0.to_radians().cos()),
        )
    }

    #[test]
    fn distance_matches_known_values() {
        assert_eq!(distance_km(53.0, 10.0, 53.0, 10.0), 0.0);
        // One degree of latitude is about 111.2 km everywhere.
        assert!((distance_km(53.0, 10.0, 54.0, 10.0) - 111.195).abs() < 0.01);
        // One degree of longitude shrinks with cos(latitude).
        assert!((distance_km(60.0, 10.0, 60.0, 11.0) - 55.6).abs() < 0.1);
        // Hamburg airport to Copenhagen airport is roughly 290 km.
        let d = distance_km(53.6304, 9.9882, 55.6180, 12.6508);
        assert!((d - 281.0).abs() < 5.0, "{d}");
    }

    #[test]
    fn track_is_formatted_on_a_360_scale() {
        assert_eq!(format_track(0.0), "360°");
        assert_eq!(format_track(359.7), "360°");
        assert_eq!(format_track(0.4), "360°");
        assert_eq!(format_track(1.0), "001°");
        assert_eq!(format_track(90.0), "090°");
        assert_eq!(format_track(224.6), "225°");
    }

    #[test]
    fn position_inside_rectangle_starts_a_pass() {
        let mut f = filter();
        let now = Instant::now();
        let (lat, lon) = offset(3.0, 0.0);
        assert!(f.add_position(
            1,
            Fix {
                time: 0.0,
                lat,
                lon
            },
            Some(2000),
            now
        ));
        let pass = &f.passes[&1];
        assert!(
            (pass.distance_km - 3.0).abs() < 0.01,
            "{}",
            pass.distance_km
        );
        assert_eq!(f.total_passes, 1);
    }

    #[test]
    fn position_outside_rectangle_is_ignored() {
        let mut f = filter();
        let (lat, lon) = offset(5.2, 0.0);
        assert!(!f.add_position(
            1,
            Fix {
                time: 0.0,
                lat,
                lon
            },
            Some(2000),
            Instant::now()
        ));
        assert!(f.passes.is_empty());
        assert_eq!(f.total_passes, 0);
    }

    /// Whether a point `east_km` and `north_km` from the centre is inside.
    fn inside(f: &Filter, east_km: f64, north_km: f64) -> bool {
        let (lat, lon) = offset(east_km, north_km);
        f.contains(lat, lon)
    }

    #[test]
    fn rectangle_reaches_into_its_corners() {
        let f = filter();
        assert!(inside(&f, 0.0, 0.0), "the point itself is the centre");
        assert!(inside(&f, 4.9, 4.9), "corner, 6.9 km from the centre");
        assert!(inside(&f, -4.9, -4.9));
        assert!(!inside(&f, 5.1, 0.0), "just past the east edge");
        assert!(!inside(&f, 0.0, -5.1), "just past the south edge");
    }

    #[test]
    fn rectangle_is_turned_to_the_view_direction() {
        let f = Filter::new(CENTER, square(45.0), 5000, SECS_30);
        // Looking north-east the corners point north, east, south and west.
        assert!(inside(&f, 0.0, 7.0), "north corner");
        assert!(inside(&f, -7.0, 0.0), "west corner");
        assert!(!inside(&f, 4.9, 4.9), "7 km along the view, past the edge");
        assert!(inside(&f, 3.4, 3.4), "4.8 km along the view");
        assert!(!inside(&f, 3.7, -3.7), "past the edge to the right");

        // The default: 153 degrees, roughly south-south-east.
        let f = Filter::new(CENTER, square(153.0), 5000, SECS_30);
        let (sin, cos) = 153f64.to_radians().sin_cos();
        assert!(inside(&f, 4.9 * sin, 4.9 * cos), "4.9 km ahead");
        assert!(!inside(&f, 5.1 * sin, 5.1 * cos), "5.1 km ahead");
        assert!(inside(&f, -4.9 * sin, -4.9 * cos), "4.9 km behind");
        assert!(inside(&f, 4.9 * cos, -4.9 * sin), "4.9 km to the side");
        assert!(!inside(&f, 5.1 * cos, -5.1 * sin), "5.1 km to the side");
        assert!(!inside(&f, 4.9, 4.9), "no longer a corner");
    }

    #[test]
    fn width_and_length_are_set_separately() {
        // Looking north: 4 km wide, 20 km long.
        let area = Area {
            width_km: 4.0,
            length_km: 20.0,
            ..square(0.0)
        };
        let f = Filter::new(CENTER, area, 5000, SECS_30);
        assert!(inside(&f, 0.0, 9.9), "9.9 km ahead");
        assert!(inside(&f, 0.0, -9.9), "9.9 km behind");
        assert!(!inside(&f, 0.0, 10.1));
        assert!(inside(&f, 1.9, 0.0), "1.9 km to the side");
        assert!(!inside(&f, 2.1, 0.0), "2.1 km to the side");

        // Looking east the long edge runs west to east.
        let f = Filter::new(
            CENTER,
            Area {
                view_direction_deg: 90.0,
                ..area
            },
            5000,
            SECS_30,
        );
        assert!(inside(&f, 9.9, 0.0));
        assert!(!inside(&f, 0.0, 2.1));
    }

    #[test]
    fn offset_shifts_the_rectangle_along_the_view_direction() {
        let looking_north = |offset_percent| {
            let area = Area {
                offset_percent,
                ..square(0.0)
            };
            Filter::new(CENTER, area, 5000, SECS_30)
        };

        // All of it ahead: the point lies on the near edge.
        let f = looking_north(100.0);
        assert!(inside(&f, 0.0, 9.9));
        assert!(!inside(&f, 0.0, 10.1));
        assert!(inside(&f, 0.0, 0.0));
        assert!(!inside(&f, 0.0, -0.1), "nothing behind");
        assert!(inside(&f, 4.9, 5.0), "the width stays centred");
        assert!(!inside(&f, 5.1, 5.0));

        // All of it behind.
        let f = looking_north(0.0);
        assert!(inside(&f, 0.0, -9.9));
        assert!(!inside(&f, 0.0, 0.1), "nothing ahead");

        // Three quarters ahead, a quarter behind.
        let f = looking_north(75.0);
        assert!(inside(&f, 0.0, 7.4));
        assert!(!inside(&f, 0.0, 7.6));
        assert!(inside(&f, 0.0, -2.4));
        assert!(!inside(&f, 0.0, -2.6));
    }

    #[test]
    fn flights_are_ordered_by_entry() {
        let mut f = filter();
        let now = Instant::now();
        let (lat, lon) = offset(1.0, 1.0);
        let fix = |time| Fix { time, lat, lon };
        for (i, icao) in [30, 10, 20].into_iter().enumerate() {
            f.add_position(icao, fix(i as f64), Some(1000), now);
        }
        let order = |f: &Filter| f.flights().iter().map(|x| x.icao).collect::<Vec<_>>();
        assert_eq!(order(&f), [30, 10, 20]);

        // Another position does not change the order.
        f.add_position(30, fix(5.0), Some(1000), now);
        assert_eq!(order(&f), [30, 10, 20]);

        // Leaving and coming back puts it at the end.
        f.add_position(30, fix(6.0), Some(9000), now);
        f.add_position(30, fix(7.0), Some(1000), now);
        assert_eq!(order(&f), [10, 20, 30]);
    }

    #[test]
    fn altitude_limit_is_applied() {
        let mut f = filter();
        let now = Instant::now();
        let (lat, lon) = offset(1.0, 1.0);
        let fix = Fix {
            time: 0.0,
            lat,
            lon,
        };
        assert!(!f.add_position(1, fix, Some(5025), now), "above the limit");
        assert!(f.add_position(2, fix, Some(5000), now), "at the limit");
        assert!(f.add_position(3, fix, Some(-200), now), "below sea level");
        assert!(f.add_position(4, fix, None, now), "unknown altitude counts");
        assert_eq!(f.passes.keys().copied().collect::<Vec<_>>(), [2, 3, 4]);
    }

    /// Feed one position of aircraft 7 at `east` km, 1 km north of centre.
    fn fly(f: &mut Filter, east: f64, time: f64) -> bool {
        let (lat, lon) = offset(east, 1.0);
        f.add_position(7, Fix { time, lat, lon }, Some(3000), Instant::now())
    }

    #[test]
    fn aircraft_is_listed_exactly_while_inside_the_rectangle() {
        let mut f = filter();
        // Fly west to east straight through the rectangle.
        assert!(!fly(&mut f, -6.0, 0.0), "approaching: not listed");
        assert!(f.flights().is_empty());

        for (i, east) in [-4.0, -2.0, 0.0, 2.0, 4.0].into_iter().enumerate() {
            assert!(fly(&mut f, east, 10.0 + i as f64 * 10.0));
            assert_eq!(f.flights().len(), 1, "listed at {east} km");
        }
        let pass = &f.passes[&7];
        assert_eq!(pass.fixes, 5);
        assert!(
            (pass.min_distance_km - 1.0).abs() < 0.01,
            "{}",
            pass.min_distance_km
        );
        assert!(
            (pass.distance_km - 4.12).abs() < 0.01,
            "{}",
            pass.distance_km
        );

        assert!(fly(&mut f, 6.0, 60.0), "leaving changes the listing");
        assert!(
            f.flights().is_empty(),
            "removed at once, without waiting for expiry"
        );
        assert!(!fly(&mut f, 8.0, 70.0), "further away: nothing to change");
        assert_eq!(f.total_passes, 1);

        // Turning back in is a new pass.
        assert!(fly(&mut f, 3.0, 120.0));
        assert_eq!(f.total_passes, 2);
        assert_eq!(f.passes[&7].fixes, 1);
    }

    #[test]
    fn climbing_above_the_limit_removes_the_aircraft() {
        let mut f = filter();
        let now = Instant::now();
        let (lat, lon) = offset(0.5, 0.5);
        f.add_position(
            1,
            Fix {
                time: 0.0,
                lat,
                lon,
            },
            Some(4900),
            now,
        );
        assert_eq!(f.flights().len(), 1);
        assert!(f.add_position(
            1,
            Fix {
                time: 1.0,
                lat,
                lon
            },
            Some(5100),
            now
        ));
        assert!(f.flights().is_empty());
    }

    #[test]
    fn flights_carry_what_the_aircraft_broadcasts() {
        use crate::live::Aircraft;

        let mut f = filter();
        let (lat, lon) = offset(3.0, 0.0);
        f.add_position(
            0x405f12,
            Fix {
                time: 0.0,
                lat,
                lon,
            },
            Some(1200),
            Instant::now(),
        );
        // Position known, nothing else yet.
        let bare = &f.flights()[0];
        assert_eq!(
            (bare.icao, &bare.callsign, bare.track_deg),
            (0x405f12, &None, None)
        );
        assert!((bare.distance_km - 3.0).abs() < 0.01);

        f.tracker.rows.insert(
            0x405f12,
            Aircraft {
                callsign: Some("EZY32GY".to_string()),
                altitude_ft: Some(1200),
                track_deg: Some(153.0),
                groundspeed_kt: Some(151.0),
                ..Aircraft::default()
            },
        );
        let flight = &f.flights()[0];
        assert_eq!(flight.info, AircraftInfo::default(), "no database: no type");
        assert_eq!(flight.callsign.as_deref(), Some("EZY32GY"));
        assert_eq!(flight.altitude_ft, Some(1200));
        assert_eq!(flight.track_deg, Some(153.0));
        assert_eq!(flight.groundspeed_kt, Some(151.0));

        // With the database the type and registration are resolved.
        let f = f.with_lookup(Lookup::ready(crate::aircraft_db::tests::sample_db()));
        let info = &f.flights()[0].info;
        assert_eq!(info.typecode.as_deref(), Some("A320"));
        assert_eq!(info.registration.as_deref(), Some("G-EZTB"));
        assert_eq!(info.type_name.as_deref(), Some("AIRBUS A-320"));
    }

    #[test]
    fn silent_aircraft_expires_after_the_timeout() {
        let mut f = filter();
        let start = Instant::now();
        let (lat, lon) = offset(1.0, 0.0);
        f.add_position(
            1,
            Fix {
                time: 0.0,
                lat,
                lon,
            },
            Some(1000),
            start,
        );
        f.add_position(
            2,
            Fix {
                time: 0.0,
                lat,
                lon,
            },
            Some(1000),
            start + Duration::from_secs(20),
        );

        assert!(!View::expire(&mut f, start + Duration::from_secs(30)));
        assert!(View::expire(&mut f, start + Duration::from_secs(31)));
        assert_eq!(f.passes.keys().copied().collect::<Vec<_>>(), [2]);
        assert_eq!(f.total_passes, 2, "the count of passes is kept");

        // Coming back later is a new pass.
        f.add_position(
            1,
            Fix {
                time: 90.0,
                lat,
                lon,
            },
            Some(1000),
            start + Duration::from_secs(90),
        );
        assert_eq!(f.total_passes, 3);
        assert_eq!(f.passes[&1].fixes, 1);
    }

    #[test]
    fn decoded_frames_feed_the_filter() {
        // The reference position pair resolves to 52.2572 N, 3.9194 E at
        // 38000 ft. Centre the rectangle there and allow that altitude.
        let mut f = Filter::new((52.2572, 3.9194), square(0.0), 40000, SECS_30);
        let now = Instant::now();
        assert!(
            !View::update(&mut f, &timed(POS_ODD, 1.0), now),
            "no position yet"
        );
        assert!(View::update(&mut f, &timed(POS_EVEN, 2.0), now));
        let pass = &f.passes[&0x40621d];
        assert_eq!(pass.fixes, 1);
        assert!(pass.distance_km < 0.1, "{}", pass.distance_km);

        // An unrelated aircraft without a position is not listed.
        assert!(!View::update(&mut f, &timed(IDENT, 3.0), now));
        assert_eq!(f.passes.len(), 1);

        // Same aircraft, default altitude limit: 38000 ft is filtered out.
        let mut low = Filter::new((52.2572, 3.9194), square(0.0), 5000, SECS_30);
        View::update(&mut low, &timed(POS_ODD, 1.0), now);
        assert!(!View::update(&mut low, &timed(POS_EVEN, 2.0), now));
        assert!(low.passes.is_empty());
    }

    #[test]
    fn draw_lists_passing_aircraft_with_track() {
        use crate::aircraft_db::tests::sample_db;
        use crate::live::Aircraft;

        let mut f = filter().with_lookup(Lookup::ready(sample_db()));
        let start = Instant::now();
        // South-west bound, straight over the point.
        for (i, d) in [3.0, 2.0, 1.0, 0.0, -1.0].into_iter().enumerate() {
            let (lat, lon) = offset(d, d);
            f.add_position(
                0x3c0ac5,
                Fix {
                    time: i as f64 * 5.0,
                    lat,
                    lon,
                },
                Some(1900),
                start,
            );
        }
        // What the aircraft broadcast about itself.
        f.tracker.rows.insert(
            0x3c0ac5,
            Aircraft {
                callsign: Some("CFG5YW".to_string()),
                altitude_ft: Some(1900),
                track_deg: Some(224.6),
                groundspeed_kt: Some(151.2),
                ..Aircraft::default()
            },
        );

        let lines = screen(&f, &mut 0, start + Duration::from_secs(4), 150, 8);
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
        let lines = screen(&filter(), &mut 0, Instant::now(), 120, 8);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("no aircraft in the rectangle")),
            "{lines:#?}"
        );
    }

    #[test]
    fn arguments_have_the_expected_defaults_and_limits() {
        use clap::Parser;
        #[derive(Parser)]
        struct Wrapper {
            #[command(flatten)]
            args: FilterParams,
        }
        let parse = |extra: &[&str]| Wrapper::try_parse_from(["test"].iter().chain(extra));

        let args = parse(&[]).unwrap().args;
        assert_eq!((args.lat, args.lon), CENTER);
        assert_eq!(
            (
                args.area_width,
                args.area_length,
                args.area_offset,
                args.view_direction,
                args.max_altitude,
                args.expire
            ),
            (4.0, 6.0, 80.0, 153.0, 5000, 30)
        );

        let args = parse(&[
            "--lat",
            "-33.9",
            "--lon",
            "-151.2",
            "--area-width",
            "0.5",
            "--area-length",
            "3",
            "--area-offset",
            "80%",
        ])
        .unwrap()
        .args;
        assert_eq!((args.lat, args.lon), (-33.9, -151.2));
        assert_eq!(
            (args.area_width, args.area_length, args.area_offset),
            (0.5, 3.0, 80.0)
        );

        assert!(parse(&["--lat", "91"]).is_err());
        assert!(parse(&["--lon", "-181"]).is_err());
        assert!(parse(&["--area-width", "0"]).is_err());
        assert!(parse(&["--area-length", "-2"]).is_err());
        assert!(
            parse(&["--size", "5"]).is_err(),
            "split into width and length"
        );
        assert_eq!(
            parse(&["--area-offset", "0"]).unwrap().args.area_offset,
            0.0
        );
        assert!(parse(&["--area-offset", "101"]).is_err());
        assert!(parse(&["--area-offset", "-1"]).is_err());
        assert_eq!(
            parse(&["--view-direction", "0"])
                .unwrap()
                .args
                .view_direction,
            0.0
        );
        assert!(parse(&["--view-direction", "361"]).is_err());
        assert!(parse(&["--view-direction", "-1"]).is_err());
    }
}
