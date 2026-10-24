//! Aircraft filter: which aircraft are currently inside a rectangle around a
//! point and below an altitude limit.
//!
//! Used by `debug filter`, which lists them in a table, and by `live`, which
//! shows them on the flipdot display. Pure state: it is fed frames and asked
//! for the flights.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use rs1090::decode::TimedMessage;

use crate::domain::registry::{AircraftInfo, Registry};
use crate::domain::tracker::Tracker;

/// Mean Earth radius in kilometres.
const EARTH_RADIUS_KM: f64 = 6371.0088;
/// How long the underlying aircraft tracker keeps a silent aircraft.
const TRACKER_EXPIRE: Duration = Duration::from_secs(60);

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
    /// Distance from the point at the latest position.
    pub distance_km: f64,
    /// Closest approach to the point so far.
    pub min_distance_km: f64,
    /// Number of positions received inside the rectangle.
    pub fixes: u64,
    /// When the latest position arrived.
    pub last_fix: Instant,
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
    tracker: Tracker,
    passes: BTreeMap<u32, Pass>,
    /// Time of the last position handled per aircraft, to spot new ones.
    handled: BTreeMap<u32, f64>,
    /// Aircraft that passed since start.
    total_passes: u64,
    /// Tells registration and type from the ICAO address, if there is one.
    registry: Option<Box<dyn Registry>>,
}

impl Filter {
    pub fn new(center: (f64, f64), area: Area, max_altitude_ft: i64, expire: Duration) -> Self {
        Self {
            center,
            area,
            max_altitude_ft,
            expire,
            tracker: Tracker::new(TRACKER_EXPIRE),
            passes: BTreeMap::new(),
            handled: BTreeMap::new(),
            total_passes: 0,
            registry: None,
        }
    }

    /// Tell aircraft registration and type with `registry`.
    pub fn with_registry(mut self, registry: impl Registry + 'static) -> Self {
        self.registry = Some(Box::new(registry));
        self
    }

    /// State of the aircraft database, for status lines.
    pub fn db_status(&self) -> String {
        match &self.registry {
            Some(registry) => registry.status(),
            None => "aircraft db off".to_string(),
        }
    }

    /// Aircraft currently inside the rectangle, the one that entered first
    /// at the front.
    pub fn flights(&self) -> Vec<Flight> {
        let mut passes: Vec<_> = self.passes.iter().collect();
        passes.sort_by_key(|(_, pass)| pass.entered);
        passes
            .into_iter()
            .map(|(icao, pass)| {
                let aircraft = self.tracker.aircraft.get(icao);
                Flight {
                    icao: *icao,
                    callsign: aircraft.and_then(|a| a.callsign.clone()),
                    altitude_ft: aircraft.and_then(|a| a.altitude_ft),
                    track_deg: aircraft.and_then(|a| a.track_deg),
                    groundspeed_kt: aircraft.and_then(|a| a.groundspeed_kt),
                    distance_km: pass.distance_km,
                    min_distance_km: pass.min_distance_km,
                    fixes: pass.fixes,
                    last_fix: pass.last_fix,
                    info: self.info(*icao),
                }
            })
            .collect()
    }

    /// What the registry knows about an aircraft.
    fn info(&self, icao: u32) -> AircraftInfo {
        self.registry
            .as_ref()
            .and_then(|registry| registry.get(icao))
            .unwrap_or_default()
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

    /// The point of interest, as latitude and longitude.
    pub fn center(&self) -> (f64, f64) {
        self.center
    }

    pub fn area(&self) -> &Area {
        &self.area
    }

    /// How long an aircraft stays listed without a position report.
    pub fn pass_timeout(&self) -> Duration {
        self.expire
    }

    /// Merge one frame. Returns true if the listing changed.
    pub fn update(&mut self, msg: &TimedMessage, now: Instant) -> bool {
        let Some(icao) = self.tracker.update(msg, now) else {
            return false;
        };
        let aircraft = &self.tracker.aircraft[&icao];
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

    /// Drop aircraft that went silent. Returns true if the listing changed.
    pub fn expire(&mut self, now: Instant) -> bool {
        self.tracker.expire(now);
        let tracker = &self.tracker;
        self.handled
            .retain(|icao, _| tracker.aircraft.contains_key(icao));

        let before = self.passes.len();
        let expire = self.expire;
        self.passes
            .retain(|_, pass| now.duration_since(pass.last_fix) <= expire);
        self.passes.len() != before
    }

    /// What happened since start, in one line.
    pub fn summary(&self) -> String {
        format!(
            "{} frames received, {} aircraft entered the rectangle",
            self.tracker.frames, self.total_passes
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::domain::tracker::tests::timed;
    use crate::domain::tracker::tests::{IDENT, POS_EVEN, POS_ODD};

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
        use crate::domain::tracker::Aircraft;

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

        f.tracker.aircraft.insert(
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
        let f = f.with_registry(crate::domain::registry::tests::sample_db());
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

        assert!(!f.expire(start + Duration::from_secs(30)));
        assert!(f.expire(start + Duration::from_secs(31)));
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
        assert!(!f.update(&timed(POS_ODD, 1.0), now), "no position yet");
        assert!(f.update(&timed(POS_EVEN, 2.0), now));
        let pass = &f.passes[&0x40621d];
        assert_eq!(pass.fixes, 1);
        assert!(pass.distance_km < 0.1, "{}", pass.distance_km);

        // An unrelated aircraft without a position is not listed.
        assert!(!f.update(&timed(IDENT, 3.0), now));
        assert_eq!(f.passes.len(), 1);

        // Same aircraft, default altitude limit: 38000 ft is filtered out.
        let mut low = Filter::new((52.2572, 3.9194), square(0.0), 5000, SECS_30);
        low.update(&timed(POS_ODD, 1.0), now);
        assert!(!low.update(&timed(POS_EVEN, 2.0), now));
        assert!(low.passes.is_empty());
    }

    /// A filter that knows the aircraft database, with one aircraft that
    /// flew south-west straight over the point and was last heard at
    /// `start`.
    pub(crate) fn overflown(start: Instant) -> Filter {
        use crate::domain::registry::tests::sample_db;
        use crate::domain::tracker::Aircraft;

        let mut f = filter().with_registry(sample_db());
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
        f.tracker.aircraft.insert(
            0x3c0ac5,
            Aircraft {
                callsign: Some("CFG5YW".to_string()),
                altitude_ft: Some(1900),
                track_deg: Some(224.6),
                groundspeed_kt: Some(151.2),
                ..Aircraft::default()
            },
        );
        f
    }
}
