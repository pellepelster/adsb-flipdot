//! Options selecting which aircraft are shown.

use std::path::PathBuf;
use std::time::Duration;

use crate::cli::parse;
use crate::domain::filter::{Area, Filter};
use crate::source::aircraft_db::{Lookup, default_cache_dir};

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
    #[arg(long, value_name = "KM", default_value_t = 4.0, value_parser = parse::positive)]
    pub area_width: f64,

    /// Length of the rectangle, along the view direction, in kilometres.
    #[arg(long, value_name = "KM", default_value_t = 6.0, value_parser = parse::positive)]
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
    /// The filter these options describe. Unless it is switched off, the
    /// aircraft database starts loading in the background.
    pub fn build(&self) -> Filter {
        let filter = Filter::new(
            (self.lat, self.lon),
            Area {
                width_km: self.area_width,
                length_km: self.area_length,
                offset_percent: self.area_offset,
                view_direction_deg: self.view_direction,
            },
            self.max_altitude,
            Duration::from_secs(self.expire),
        );
        if self.db.no_aircraft_db {
            return filter;
        }
        let cache_dir = self.db.cache_dir.clone().unwrap_or_else(default_cache_dir);
        filter.with_registry(Lookup::start(cache_dir))
    }
}

/// Options for the aircraft database.
#[derive(clap::Args, Debug, Clone)]
pub struct DbArgs {
    /// Directory for downloaded data. Defaults to $XDG_CACHE_HOME/adsb-flipdot
    /// or ~/.cache/adsb-flipdot.
    #[arg(long, value_name = "DIR", env = "ADSB_FLIPDOT_CACHE_DIR")]
    pub cache_dir: Option<PathBuf>,

    /// Do not download or use the aircraft database. Aircraft type and
    /// registration are then not shown.
    #[arg(long)]
    pub no_aircraft_db: bool,
}

fn parse_latitude(s: &str) -> Result<f64, String> {
    parse::number_in(s, -90.0..=90.0)
}

fn parse_longitude(s: &str) -> Result<f64, String> {
    parse::number_in(s, -180.0..=180.0)
}

fn parse_direction(s: &str) -> Result<f64, String> {
    parse::number_in(s, 0.0..=360.0)
}

/// A percentage, with or without the sign.
fn parse_percent(s: &str) -> Result<f64, String> {
    parse::number_in(s.strip_suffix('%').unwrap_or(s), 0.0..=100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::tests::try_parse_args;

    #[test]
    fn arguments_have_the_expected_defaults_and_limits() {
        let parse = |extra: &[&str]| try_parse_args::<FilterParams>(extra);

        let args = parse(&[]).unwrap();
        assert_eq!((args.lat, args.lon), (53.7271256, 9.9065838));
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
        .unwrap();
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
        assert_eq!(parse(&["--area-offset", "0"]).unwrap().area_offset, 0.0);
        assert!(parse(&["--area-offset", "101"]).is_err());
        assert!(parse(&["--area-offset", "-1"]).is_err());
        assert_eq!(
            parse(&["--view-direction", "0"]).unwrap().view_direction,
            0.0
        );
        assert!(parse(&["--view-direction", "361"]).is_err());
        assert!(parse(&["--view-direction", "-1"]).is_err());
    }

    #[test]
    fn options_describe_the_filter() {
        let filter = try_parse_args::<FilterParams>(&[
            "--lat",
            "52.5",
            "--lon",
            "13.4",
            "--area-width",
            "8",
            "--area-length",
            "12",
            "--area-offset",
            "25",
            "--view-direction",
            "90",
            "--max-altitude",
            "3000",
            "--expire",
            "45",
            "--no-aircraft-db",
        ])
        .unwrap()
        .build();
        assert_eq!(filter.center(), (52.5, 13.4));
        assert_eq!(
            *filter.area(),
            Area {
                width_km: 8.0,
                length_km: 12.0,
                offset_percent: 25.0,
                view_direction_deg: 90.0,
            }
        );
        assert_eq!(filter.max_altitude_ft(), 3000);
        assert_eq!(filter.pass_timeout(), Duration::from_secs(45));
        assert_eq!(filter.db_status(), "aircraft db off", "switched off");
    }
}
