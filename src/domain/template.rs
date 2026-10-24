//! Display templates: text with `{{ code }}` placeholders that are filled
//! in per aircraft. The display shows the [`TEMPLATES`].
//!
//! ```text
//! {{ name }}\n{{ altitude }} {{ track }}     two lines
//! {{ callsign }} {{ aircraft code }} {{ track }}   one line
//! {{ speed }}{{ fill }}{{ altitude }}            one at each edge
//! ```

use crate::domain::filter::{Flight, format_track};
use crate::domain::scene::FILL;

/// The name of the code that splits a line in two. It has no value, so it
/// is not one of the [`CODES`].
const FILL_NAME: &str = "fill";

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

/// One code and how it is written in a template.
pub struct CodeInfo {
    pub code: Code,
    /// The name used in a template.
    pub name: &'static str,
}

/// All template codes.
pub const CODES: &[CodeInfo] = &[
    CodeInfo {
        code: Code::Name,
        name: "name",
    },
    CodeInfo {
        code: Code::Callsign,
        name: "callsign",
    },
    CodeInfo {
        code: Code::Icao,
        name: "icao",
    },
    CodeInfo {
        code: Code::AircraftType,
        name: "aircraft_code",
    },
    CodeInfo {
        code: Code::TypeName,
        name: "aircraft_name",
    },
    CodeInfo {
        code: Code::Registration,
        name: "registration",
    },
    CodeInfo {
        code: Code::Altitude,
        name: "altitude",
    },
    CodeInfo {
        code: Code::Track,
        name: "track",
    },
    CodeInfo {
        code: Code::Speed,
        name: "speed",
    },
    CodeInfo {
        code: Code::Distance,
        name: "distance",
    },
    CodeInfo {
        code: Code::Index,
        name: "index",
    },
    CodeInfo {
        code: Code::Count,
        name: "count",
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
    /// What follows goes to the right edge of the display.
    Fill,
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
                rest = &after[close + 2..];
                if normalize(name) == FILL_NAME {
                    parts.push(Part::Fill);
                    continue;
                }
                let code = Code::from_name(name).ok_or_else(|| {
                    format!(
                        "unknown template code `{name}`. Known codes: {}, {FILL_NAME}",
                        CODES.iter().map(|c| c.name).collect::<Vec<_>>().join(", ")
                    )
                })?;
                parts.push(Part::Code(code));
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
    /// up, so `{{ aircraft_code }} {{ track }}` never starts with a gap. So
    /// are the brackets around an unknown value: `({{ aircraft_code }})`.
    /// Brackets that only open the line because the values before them are
    /// unknown are dropped as well: there is nothing left to set the value
    /// apart from, so `{{ aircraft_name }} ({{ aircraft_code }})` is `A320`.
    pub fn render(&self, flight: &Flight, index: usize, count: usize) -> Rendered {
        let mut complete = true;
        let lines = self
            .lines
            .iter()
            .map(|parts| {
                let mut text = String::new();
                let (mut codes, mut known) = (0, 0);
                // A `)` that closes brackets that are dropped.
                let mut drop_bracket = false;
                // An unknown value on this side of the fill.
                let mut missing = false;
                for (i, part) in parts.iter().enumerate() {
                    match part {
                        Part::Text(t) => {
                            let t = if drop_bracket { &t[1..] } else { t };
                            text.push_str(t);
                            drop_bracket = false;
                        }
                        Part::Code(code) => {
                            codes += 1;
                            let value = code.value(flight, index, count);
                            let opens_line =
                                missing && text.rsplit(FILL).next().unwrap_or("").trim() == "(";
                            if (value.is_none() || opens_line)
                                && text.ends_with('(')
                                && matches!(parts.get(i + 1), Some(Part::Text(t)) if t.starts_with(')'))
                            {
                                text.pop();
                                drop_bracket = true;
                            }
                            if let Some(value) = value {
                                known += 1;
                                text.push_str(&value);
                            } else {
                                missing = true;
                            }
                        }
                        Part::Fill => {
                            text.push(FILL);
                            missing = false;
                        }
                    }
                }
                if codes > 0 && known == 0 {
                    complete = false;
                }
                // Close up the gaps on each side of the fill on its own.
                text.split(FILL)
                    .map(|side| side.split_whitespace().collect::<Vec<_>>().join(" "))
                    .collect::<Vec<_>>()
                    .join(&FILL.to_string())
            })
            .collect();
        Rendered { lines, complete }
    }
}

/// The pages shown for every aircraft, one after another: who it is, its
/// registration and its type on top, and speed and altitude below. A page
/// whose value on top is not known is skipped.
pub fn templates() -> Vec<Template> {
    TEMPLATES
        .iter()
        .map(|t| Template::parse(t).expect("the templates are valid"))
        .collect()
}

pub const TEMPLATES: [&str; 3] = [
    "flight {{ name }}\\n{{ speed }}{{ fill }}{{ altitude }}",
    "reg {{ registration }}\\n{{ speed }}{{ fill }}{{ altitude }}",
    "{{ aircraft_name }} ({{ aircraft_code }})\\n{{ speed }}{{ fill }}{{ altitude }}",
];

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
    fn every_code_renders_its_value() {
        let flight = easyjet();
        let values = [
            "EZY32GY",
            "EZY32GY",
            "405F12",
            "A320",
            "AIRBUS A-320",
            "G-EZTB",
            "1200ft",
            "153°",
            "151kt",
            "1.5km",
            "1",
            "2",
        ];
        assert_eq!(CODES.len(), values.len());
        for (info, expected) in CODES.iter().zip(values) {
            let value = info.code.value(&flight, 0, 2);
            assert_eq!(value.as_deref(), Some(expected), "{}", info.name);
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
    fn templates_give_the_standard_page() {
        let pages: Vec<_> = templates()
            .iter()
            .map(|t| t.render(&easyjet(), 0, 1).lines)
            .collect();
        assert_eq!(
            pages,
            [
                ["flight EZY32GY", "151kt\t1200ft"],
                ["reg G-EZTB", "151kt\t1200ft"],
                ["AIRBUS A-320 (A320)", "151kt\t1200ft"],
            ]
        );
    }

    #[test]
    fn standard_pages_of_an_unknown_aircraft_show_only_its_address() {
        let mut unknown = anonymous(0xabc123);
        unknown.altitude_ft = Some(900);
        let pages: Vec<_> = templates()
            .iter()
            .map(|t| t.render(&unknown, 0, 1))
            .collect();
        assert_eq!(pages[0].lines, ["flight ABC123", "\t900ft"]);
        assert!(pages[0].complete);
        assert!(!pages[1].complete && !pages[2].complete);
    }

    #[test]
    fn standard_type_page_shows_the_code_alone_without_brackets() {
        let mut flight = easyjet();
        flight.info.type_name = None;
        let page = templates()[2].render(&flight, 0, 1);
        assert_eq!(page.lines, ["A320", "151kt\t1200ft"]);
        assert!(page.complete);
    }

    #[test]
    fn fill_splits_the_line_and_closes_up_each_side() {
        assert_eq!(
            lines("{{ speed }} {{ fill }} {{ altitude }}", &easyjet()),
            ["151kt\t1200ft"]
        );
        assert_eq!(lines("{{ Fill }}{{ track }}", &easyjet()), ["\t153°"]);
        let mut slow = easyjet();
        slow.groundspeed_kt = None;
        assert_eq!(
            lines("{{ speed }} kt{{ fill }}{{ altitude }} ", &slow),
            ["kt\t1200ft"]
        );
    }

    #[test]
    fn brackets_are_kept_around_known_values_and_in_plain_text() {
        let mut flight = easyjet();
        assert_eq!(lines("({{ aircraft_code }})", &flight), ["(A320)"]);
        flight.info.typecode = None;
        assert_eq!(lines("a ({{ aircraft_code }}) b", &flight), ["a b"]);
        assert_eq!(lines("({{ aircraft_code }} )", &flight), ["( )"]);
        assert_eq!(lines("() {{ name }}", &flight), ["() EZY32GY"]);
    }

    #[test]
    fn brackets_that_would_open_the_line_are_dropped() {
        let mut flight = easyjet();
        flight.info.type_name = None;
        let source = "{{ aircraft_name }} ({{ aircraft_code }})";
        assert_eq!(lines(source, &flight), ["A320"]);
        assert_eq!(
            lines(&format!("{{{{ speed }}}}{{{{ fill }}}} {source}"), &flight),
            ["151kt\tA320"]
        );
        assert_eq!(lines("x ({{ aircraft_code }})", &flight), ["x (A320)"]);
        assert_eq!(
            lines(
                "{{ aircraft_name }}{{ fill }}({{ aircraft_code }})",
                &flight
            ),
            ["\t(A320)"],
            "the unknown value is on the other side"
        );
    }

    #[test]
    fn unknown_code_is_an_error_that_lists_the_codes() {
        let error = Template::parse("{{ callsign }} {{ wingspan }}").unwrap_err();
        assert!(
            error.contains("unknown template code `wingspan`"),
            "{error}"
        );
        assert!(error.contains("aircraft_code"), "{error}");
        assert!(error.contains("fill"), "{error}");
        assert!(Template::parse("{{ }}").is_err(), "empty name");
    }

    #[test]
    fn unclosed_placeholder_is_an_error() {
        let error = Template::parse("{{ callsign").unwrap_err();
        assert!(error.contains("without closing"), "{error}");
        assert!(Template::parse("{{ callsign }} {{ track }").is_err());
    }
}
