//! Display templates: text with `{{ code }}` placeholders that are filled
//! in per aircraft.
//!
//! ```text
//! {{ name }}\n{{ altitude }} {{ track }}     two lines
//! {{ callsign }} {{ aircraft code }} {{ track }}   one line
//! ```

use crate::domain::filter::{Flight, format_track};

/// A value that can be put on the display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Name,
    Callsign,
    Icao,
    AircraftType,
    TypeName,
    Registration,
    Altitude,
    Track,
    Speed,
    Distance,
    Index,
    Count,
}

/// Everything there is to know about one code: how to write it, what it
/// means and what it looks like.
pub struct CodeInfo {
    pub code: Code,
    /// The name used in a template.
    pub name: &'static str,
    pub description: &'static str,
    pub example: &'static str,
}

/// All template codes, in the order they are documented.
pub const CODES: &[CodeInfo] = &[
    CodeInfo {
        code: Code::Name,
        name: "name",
        description: "Callsign, or the ICAO address while no callsign is known",
        example: "EZY32GY",
    },
    CodeInfo {
        code: Code::Callsign,
        name: "callsign",
        description: "Callsign as broadcast, usually the flight number",
        example: "EZY32GY",
    },
    CodeInfo {
        code: Code::Icao,
        name: "icao",
        description: "24-bit ICAO address of the airframe",
        example: "405F12",
    },
    CodeInfo {
        code: Code::AircraftType,
        name: "aircraft_code",
        description: "ICAO aircraft type designator (from the aircraft database)",
        example: "A320",
    },
    CodeInfo {
        code: Code::TypeName,
        name: "aircraft_name",
        description: "Aircraft type in words (from the aircraft database)",
        example: "AIRBUS A-320",
    },
    CodeInfo {
        code: Code::Registration,
        name: "registration",
        description: "Tail number (from the aircraft database)",
        example: "G-EZTB",
    },
    CodeInfo {
        code: Code::Altitude,
        name: "altitude",
        description: "Barometric altitude in feet",
        example: "1200ft",
    },
    CodeInfo {
        code: Code::Track,
        name: "track",
        description: "Direction of flight over ground, 001 to 360",
        example: "153°",
    },
    CodeInfo {
        code: Code::Speed,
        name: "speed",
        description: "Speed over ground in knots",
        example: "151kt",
    },
    CodeInfo {
        code: Code::Distance,
        name: "distance",
        description: "Distance from the point of interest in kilometres",
        example: "1.5km",
    },
    CodeInfo {
        code: Code::Index,
        name: "index",
        description: "Position of this aircraft among those in the rectangle",
        example: "1",
    },
    CodeInfo {
        code: Code::Count,
        name: "count",
        description: "Number of aircraft in the rectangle",
        example: "2",
    },
];

/// `Aircraft Code`, `aircraft-code` and `aircraft_code` are the same name.
fn normalize(name: &str) -> String {
    name.split(|c: char| c.is_whitespace() || c == '-' || c == '_')
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("_")
}

impl Code {
    fn from_name(name: &str) -> Option<Self> {
        let name = normalize(name);
        CODES
            .iter()
            .find(|info| info.name == name)
            .map(|info| info.code)
    }

    /// The value for `flight`, or `None` when it is not known.
    fn value(self, flight: &Flight, index: usize, count: usize) -> Option<String> {
        let non_empty = |s: &Option<String>| s.clone().filter(|v| !v.is_empty());
        match self {
            Code::Name => non_empty(&flight.callsign).or(Some(format!("{:06X}", flight.icao))),
            Code::Callsign => non_empty(&flight.callsign),
            Code::Icao => Some(format!("{:06X}", flight.icao)),
            Code::AircraftType => non_empty(&flight.info.typecode),
            Code::TypeName => non_empty(&flight.info.type_name),
            Code::Registration => non_empty(&flight.info.registration),
            Code::Altitude => flight.altitude_ft.map(|v| format!("{v}ft")),
            Code::Track => flight.track_deg.map(format_track),
            Code::Speed => flight.groundspeed_kt.map(|v| format!("{v:.0}kt")),
            Code::Distance => Some(format!("{:.1}km", flight.distance_km)),
            Code::Index => Some((index + 1).to_string()),
            Code::Count => Some(count.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    Code(Code),
}

/// A parsed template: one list of parts per display line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    lines: Vec<Vec<Part>>,
}

/// A template filled in for one aircraft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub lines: Vec<String>,
    /// False when some line had placeholders and none of them had a value.
    /// Such a page says nothing and is skipped when another page can be
    /// shown instead.
    pub complete: bool,
}

impl Template {
    /// Parse a template. `\n`, typed literally or as a real newline, starts
    /// a new display line.
    pub fn parse(source: &str) -> Result<Self, String> {
        let source = source.replace("\\n", "\n");
        let mut lines = Vec::new();
        for line in source.split('\n') {
            let mut parts = Vec::new();
            let mut rest = line;
            while let Some(open) = rest.find("{{") {
                if open > 0 {
                    parts.push(Part::Text(rest[..open].to_string()));
                }
                let after = &rest[open + 2..];
                let close = after
                    .find("}}")
                    .ok_or_else(|| format!("`{{{{` without closing `}}}}` in `{line}`"))?;
                let name = after[..close].trim();
                let code = Code::from_name(name).ok_or_else(|| {
                    format!(
                        "unknown template code `{name}`. Known codes: {}. \
                         Run `adsb-flipdot template-codes` for details",
                        CODES.iter().map(|c| c.name).collect::<Vec<_>>().join(", ")
                    )
                })?;
                parts.push(Part::Code(code));
                rest = &after[close + 2..];
            }
            if !rest.is_empty() {
                parts.push(Part::Text(rest.to_string()));
            }
            lines.push(parts);
        }
        Ok(Self { lines })
    }

    /// Fill in the template for `flight`, which is number `index` (from 0)
    /// of `count` aircraft in the rectangle.
    ///
    /// Unknown values are left out, and the spaces around them are closed
    /// up, so `{{ aircraft_code }} {{ track }}` never starts with a gap.
    pub fn render(&self, flight: &Flight, index: usize, count: usize) -> Rendered {
        let mut complete = true;
        let lines = self
            .lines
            .iter()
            .map(|parts| {
                let mut text = String::new();
                let (mut codes, mut known) = (0, 0);
                for part in parts {
                    match part {
                        Part::Text(t) => text.push_str(t),
                        Part::Code(code) => {
                            codes += 1;
                            if let Some(value) = code.value(flight, index, count) {
                                known += 1;
                                text.push_str(&value);
                            }
                        }
                    }
                }
                if codes > 0 && known == 0 {
                    complete = false;
                }
                text.split_whitespace().collect::<Vec<_>>().join(" ")
            })
            .collect();
        Rendered { lines, complete }
    }
}

/// The templates used when none is given: two pages that alternate.
pub fn default_templates() -> Vec<Template> {
    DEFAULT_TEMPLATES
        .iter()
        .map(|t| Template::parse(t).expect("default templates are valid"))
        .collect()
}

pub const DEFAULT_TEMPLATES: [&str; 2] = [
    "{{ name }}\\n{{ altitude }} {{ track }}",
    "{{ name }}\\n{{ aircraft_code }} {{ registration }}",
];

/// The text printed by `adsb-flipdot template-codes`.
pub fn help() -> String {
    let name_width = CODES.iter().map(|c| c.name.len()).max().unwrap_or(0) + 6;
    let example_width = CODES
        .iter()
        .map(|c| c.example.chars().count())
        .max()
        .unwrap_or(0);

    let mut out = String::from("Template codes for `adsb-flipdot run --template`\n\n");
    out.push_str(&format!(
        "  {:<name_width$}  {:<example_width$}  {}\n",
        "CODE", "EXAMPLE", "MEANING"
    ));
    for info in CODES {
        let code = format!("{{{{ {} }}}}", info.name);
        out.push_str(&format!(
            "  {code:<name_width$}  {:<example_width$}  {}\n",
            info.example, info.description
        ));
    }
    out.push_str(&format!(
        "
Rules
  - Names ignore case, and spaces or dashes count as underscores:
    {{{{ aircraft code }}}} is the same as {{{{ aircraft_code }}}}.
  - \\n starts a second display line.
  - A value that is not known is left out, together with the gap it leaves.
  - A line wider than the display scrolls. Its values are refreshed once
    per pass. `--scroll always` also scrolls lines that would fit.
  - Give --template several times to alternate between pages. A page whose
    line has no known value at all is skipped.
  - Fonts cover ASCII and Western European characters; others show as `?`.

Defaults
  --template '{}'
  --template '{}'

Examples
  adsb-flipdot run --template '{{{{ callsign }}}} {{{{ aircraft code }}}} {{{{ track }}}}'
  adsb-flipdot run --template '{{{{ name }}}}\\n{{{{ aircraft_name }}}} {{{{ altitude }}}} {{{{ speed }}}}'
",
        DEFAULT_TEMPLATES[0], DEFAULT_TEMPLATES[1]
    ));
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::domain::registry::AircraftInfo;

    /// A fully known flight.
    pub(crate) fn easyjet() -> Flight {
        Flight {
            icao: 0x405f12,
            callsign: Some("EZY32GY".to_string()),
            altitude_ft: Some(1200),
            track_deg: Some(153.2),
            groundspeed_kt: Some(151.0),
            distance_km: 1.5,
            min_distance_km: 1.5,
            fixes: 1,
            last_fix: std::time::Instant::now(),
            info: AircraftInfo {
                registration: Some("G-EZTB".to_string()),
                typecode: Some("A320".to_string()),
                type_name: Some("AIRBUS A-320".to_string()),
            },
        }
    }

    /// A flight of which only the position is known.
    pub(crate) fn anonymous(icao: u32) -> Flight {
        Flight {
            icao,
            callsign: None,
            altitude_ft: None,
            track_deg: None,
            groundspeed_kt: None,
            distance_km: 4.25,
            min_distance_km: 4.25,
            fixes: 1,
            last_fix: std::time::Instant::now(),
            info: AircraftInfo::default(),
        }
    }

    fn render(template: &str, flight: &Flight) -> Rendered {
        Template::parse(template).unwrap().render(flight, 0, 2)
    }

    fn lines(template: &str, flight: &Flight) -> Vec<String> {
        render(template, flight).lines
    }

    #[test]
    fn every_code_renders_its_documented_example() {
        let flight = easyjet();
        for info in CODES {
            let value = info.code.value(&flight, 0, 2);
            assert_eq!(value.as_deref(), Some(info.example), "{}", info.name);
        }
    }

    #[test]
    fn every_name_parses_to_its_code() {
        let mut seen = std::collections::BTreeSet::new();
        for info in CODES {
            let name = info.name;
            assert_eq!(Code::from_name(name), Some(info.code), "{name}");
            assert!(seen.insert(name), "name `{name}` is used twice");
            assert_eq!(normalize(name), name, "names are stored normalized");
        }
    }

    #[test]
    fn a_code_has_no_second_name() {
        for name in ["type", "typecode", "flight", "hex", "reg", "alt", "dist"] {
            let error = Template::parse(&format!("{{{{ {name} }}}}")).unwrap_err();
            assert!(error.contains("unknown template code"), "{name}: {error}");
        }
    }

    #[test]
    fn the_users_example_works_as_written() {
        assert_eq!(
            lines("{{ aircraft code }} {{ track }}", &easyjet()),
            ["A320 153°"]
        );
    }

    #[test]
    fn names_ignore_case_spacing_and_dashes() {
        for name in [
            "aircraft_code",
            "Aircraft Code",
            "AIRCRAFT-CODE",
            "  aircraft   code  ",
        ] {
            assert_eq!(
                lines(&format!("{{{{{name}}}}}"), &easyjet()),
                ["A320"],
                "{name}"
            );
        }
    }

    #[test]
    fn literal_text_is_kept() {
        assert_eq!(
            lines("Flight {{ callsign }} at {{ altitude }}!", &easyjet()),
            ["Flight EZY32GY at 1200ft!"]
        );
        assert_eq!(
            lines("no codes { here }", &easyjet()),
            ["no codes { here }"]
        );
    }

    #[test]
    fn newline_starts_a_second_line() {
        let expected = ["EZY32GY", "1200ft 153°"];
        assert_eq!(
            lines("{{ name }}\\n{{ altitude }} {{ track }}", &easyjet()),
            expected
        );
        assert_eq!(
            lines("{{ name }}\n{{ altitude }} {{ track }}", &easyjet()),
            expected
        );
    }

    #[test]
    fn unknown_values_are_left_out_without_gaps() {
        let mut flight = easyjet();
        flight.info.typecode = None;
        assert_eq!(
            lines("{{ callsign }} {{ aircraft_code }} {{ track }}", &flight),
            ["EZY32GY 153°"]
        );
        assert_eq!(lines("{{ aircraft_code }} {{ track }}", &flight), ["153°"]);
        assert_eq!(lines("{{ track }} {{ aircraft_code }}", &flight), ["153°"]);
    }

    #[test]
    fn name_falls_back_to_the_icao_address() {
        let flight = anonymous(0x00ab12);
        assert_eq!(
            lines("{{ name }}|{{ callsign }}|{{ icao }}", &flight),
            ["00AB12||00AB12"]
        );
        let mut empty = anonymous(0x00ab12);
        empty.callsign = Some(String::new());
        assert_eq!(lines("{{ name }}", &empty), ["00AB12"], "empty callsign");
    }

    #[test]
    fn index_and_count_describe_the_position_in_the_rectangle() {
        let template = Template::parse("{{ index }}/{{ count }}").unwrap();
        assert_eq!(template.render(&easyjet(), 0, 3).lines, ["1/3"]);
        assert_eq!(template.render(&easyjet(), 2, 3).lines, ["3/3"]);
    }

    #[test]
    fn page_is_incomplete_when_a_line_has_no_known_value() {
        let unknown = anonymous(1);
        // Line 2 consists only of values the database would provide.
        let page = render(
            "{{ name }}\\n{{ aircraft_code }} {{ registration }}",
            &unknown,
        );
        assert_eq!(page.lines, ["000001", ""]);
        assert!(!page.complete);

        // One known value on the line is enough.
        let mut partial = anonymous(1);
        partial.info.registration = Some("G-EZTB".to_string());
        let page = render(
            "{{ name }}\\n{{ aircraft_code }} {{ registration }}",
            &partial,
        );
        assert_eq!(page.lines, ["000001", "G-EZTB"]);
        assert!(page.complete);

        // Lines without placeholders never make a page incomplete.
        assert!(render("hello\\n\\n{{ name }}", &unknown).complete);
        assert!(render("", &unknown).complete);
        // Literal text does not rescue a line whose values are all missing.
        assert!(!render("Type: {{ aircraft_code }}", &unknown).complete);
    }

    #[test]
    fn default_templates_give_the_two_standard_pages() {
        let pages: Vec<_> = default_templates()
            .iter()
            .map(|t| t.render(&easyjet(), 0, 1).lines)
            .collect();
        assert_eq!(pages[0], ["EZY32GY", "1200ft 153°"]);
        assert_eq!(pages[1], ["EZY32GY", "A320 G-EZTB"]);
    }

    #[test]
    fn unknown_code_is_an_error_that_lists_the_codes() {
        let error = Template::parse("{{ callsign }} {{ wingspan }}").unwrap_err();
        assert!(
            error.contains("unknown template code `wingspan`"),
            "{error}"
        );
        assert!(error.contains("aircraft_code"), "{error}");
        assert!(error.contains("template-codes"), "{error}");
        assert!(Template::parse("{{ }}").is_err(), "empty name");
    }

    #[test]
    fn unclosed_placeholder_is_an_error() {
        let error = Template::parse("{{ callsign").unwrap_err();
        assert!(error.contains("without closing"), "{error}");
        assert!(Template::parse("{{ callsign }} {{ track }").is_err());
    }

    #[test]
    fn help_lists_every_code_with_example() {
        let help = help();
        for info in CODES {
            assert!(
                help.contains(&format!("{{{{ {} }}}}", info.name)),
                "{}",
                info.name
            );
            assert!(help.contains(info.example), "{}", info.example);
            assert!(help.contains(info.description), "{}", info.description);
        }
        assert!(help.contains("{{ aircraft code }} is the same as {{ aircraft_code }}"));
        assert!(help.contains(DEFAULT_TEMPLATES[0]));
    }

    #[test]
    fn examples_in_the_help_text_are_valid_templates() {
        let help = help();
        let mut checked = 0;
        for line in help.lines().filter(|l| l.contains("--template '")) {
            let template = line.split('\'').nth(1).unwrap();
            Template::parse(template).unwrap_or_else(|e| panic!("{template}: {e}"));
            checked += 1;
        }
        assert_eq!(checked, 4, "two defaults and two examples");
    }
}
