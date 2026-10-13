//! `run`: show aircraft passing the point of interest on the flipdot display.
//!
//! Frames go through the same [`Filter`] as `debug filter`. Each aircraft
//! inside the rectangle is turned into text by one or more [`Template`]s, and
//! the text is laid out by the same [`Scene`] as `debug flipdot`. For now
//! the display is simulated in the terminal.

use std::time::Instant;

use anyhow::Result;

use crate::display::{Display, Note, Stacking, Terminal};
use crate::filter::{Filter, FilterParams, Flight, format_track};
use crate::flipdot::{DisplayArgs, FrameBuffer, Scene};
use crate::source::{FrameStream, SourceArgs};
use crate::template::{Template, default_templates};
use crate::tui::View;

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

/// The page on the display: one template filled in for one aircraft.
struct Page {
    icao: u32,
    template: usize,
    lines: Vec<String>,
    scene: Scene,
    /// Ticks since this page came up.
    elapsed: u64,
    /// Ticks since the scene was laid out; drives the scrolling.
    tick: u64,
}

/// Decides what is on the display at every tick: which aircraft, which
/// template, and how far a scrolling line has moved. Pure state, no I/O.
///
/// Only one aircraft is shown at a time: pages go round its templates
/// until it has left the rectangle, and only then the one that has been
/// inside the longest comes up. A static page shows live values. A scrolling page keeps its
/// values for one pass, because text that changes under way would jump.
struct Presenter {
    templates: Vec<Template>,
    /// Minimum time a page stays up, in ticks.
    page_ticks: u64,
    page: Option<Page>,
}

impl Presenter {
    fn new(templates: Vec<Template>, page_ticks: u64) -> Self {
        Self {
            templates,
            page_ticks: page_ticks.max(1),
            page: None,
        }
    }

    /// The aircraft on the display, as its position in `flights`.
    fn shown(&self, flights: &[Flight]) -> Option<usize> {
        let icao = self.page.as_ref()?.icao;
        flights.iter().position(|f| f.icao == icao)
    }

    /// The dots to show now.
    fn frame(&self, display: &DisplayArgs) -> FrameBuffer {
        match &self.page {
            Some(page) => page.scene.frame(page.tick),
            None => {
                let (width, height) = display.display_size();
                FrameBuffer::new(width, height)
            }
        }
    }

    /// Bring up the first page of aircraft `aircraft` worth showing, trying
    /// template `template` first and then every other one. If no page has
    /// anything to say, the first template is shown regardless.
    fn open(
        &mut self,
        flights: &[Flight],
        display: &DisplayArgs,
        aircraft: usize,
        template: usize,
    ) {
        let (count, templates) = (flights.len(), self.templates.len());
        let flight = &flights[aircraft];
        let pick = (0..templates)
            .map(|step| (template + step) % templates)
            .find(|&t| self.templates[t].render(flight, aircraft, count).complete)
            .unwrap_or(0);

        let lines = self.templates[pick].render(flight, aircraft, count).lines;
        self.page = Some(Page {
            icao: flight.icao,
            template: pick,
            scene: display.scene(&lines),
            lines,
            elapsed: 0,
            tick: 0,
        });
    }

    /// Advance one tick. Returns true when the text on the display changed,
    /// as opposed to a scrolling line merely moving on.
    fn step(&mut self, flights: &[Flight], display: &DisplayArgs) -> bool {
        if flights.is_empty() {
            return self.page.take().is_some();
        }
        let Some(page) = &mut self.page else {
            self.open(flights, display, 0, 0);
            return true;
        };
        let Some(index) = flights.iter().position(|f| f.icao == page.icao) else {
            // The aircraft left: on to the one that has waited longest.
            self.open(flights, display, 0, 0);
            return true;
        };

        page.elapsed += 1;
        page.tick += 1;
        // A scrolling page may only change between two passes.
        let cycle = page.scene.cycle_ticks();
        if cycle.is_some_and(|cycle| page.tick % cycle != 0) {
            return false;
        }
        if page.elapsed >= self.page_ticks {
            let (template, lines) = (page.template + 1, page.lines.clone());
            self.open(flights, display, index, template);
            // With a single page worth showing the text may be the same.
            return self.page.as_ref().is_some_and(|page| page.lines != lines);
        }

        // Same page, fresh values.
        let lines = self.templates[page.template]
            .render(&flights[index], index, flights.len())
            .lines;
        if cycle.is_some() {
            // Start the next pass, whether or not the text changed.
            page.tick = 0;
        }
        if lines == page.lines {
            return false;
        }
        page.scene = display.scene(&lines);
        page.lines = lines;
        page.tick = 0;
        true
    }
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
    let templates = if args.templates.is_empty() {
        default_templates()
    } else {
        args.templates
    };
    // One tick moves a scrolling line by one dot.
    let mut presenter = Presenter::new(templates, args.page * u64::from(args.display.speed));
    let mut filter = args.filter.build();
    let mut stream = FrameStream::open(&args.source).await?;
    eprintln!("adsb-flipdot: {}", stream.description());

    // The simulated display in the terminal: dots plus a status line.
    let (width, height) = args.display.display_size();
    let mut display = Display::new(width, height);
    display.attach(Terminal::stdout(width, height, Stacking::InPlace));
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let mut tick = tokio::time::interval(args.display.frame_time());
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut first = true;
    loop {
        tokio::select! {
            _ = &mut ctrl_c => break,
            msg = stream.recv() => match msg {
                Some(msg) => {
                    filter.update(&msg, Instant::now());
                }
                None => break,
            },
            _ = tick.tick() => {
                filter.expire(Instant::now());
                let flights = filter.flights();
                let changed = presenter.step(&flights, &args.display) || first;
                first = false;
                let status = status_line(&filter, &flights, presenter.shown(&flights));
                let note = Note {
                    caption: Some(&status),
                    new_text: changed,
                };
                display.show(&presenter.frame(&args.display), note)?;
            }
        }
    }

    let result = stream.close().await;
    eprintln!("adsb-flipdot: {}", filter.summary());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Area;
    use crate::template::tests::{anonymous, easyjet};
    use std::time::Duration;

    /// Two 28x14 panels, 5x7 font, 10 dots per second: 11 characters fit.
    fn display(extra: &[&str]) -> DisplayArgs {
        use clap::Parser;
        #[derive(Parser)]
        struct Wrapper {
            #[command(flatten)]
            args: DisplayArgs,
        }
        Wrapper::parse_from(["test"].iter().chain(extra)).args
    }

    fn templates(sources: &[&str]) -> Vec<Template> {
        sources
            .iter()
            .map(|s| Template::parse(s).unwrap())
            .collect()
    }

    /// A known aircraft with the given address and callsign.
    fn flight(icao: u32, callsign: &str) -> Flight {
        Flight {
            icao,
            callsign: Some(callsign.to_string()),
            ..easyjet()
        }
    }

    fn lines(presenter: &Presenter) -> Vec<String> {
        presenter
            .page
            .as_ref()
            .map(|p| p.lines.clone())
            .unwrap_or_default()
    }

    /// Step until the text changes; returns the number of ticks it took.
    fn until_change(presenter: &mut Presenter, flights: &[Flight], display: &DisplayArgs) -> u64 {
        for ticks in 1..10_000 {
            if presenter.step(flights, display) {
                return ticks;
            }
        }
        panic!("text never changed");
    }

    #[test]
    fn display_is_dark_without_aircraft() {
        let display = display(&[]);
        let mut presenter = Presenter::new(default_templates(), 30);
        assert!(!presenter.step(&[], &display));
        assert_eq!(presenter.frame(&display), FrameBuffer::new(56, 14));
        assert_eq!(presenter.shown(&[]), None);
    }

    #[test]
    fn default_pages_alternate_for_one_aircraft() {
        let display = display(&[]);
        let flights = [easyjet()];
        let mut presenter = Presenter::new(default_templates(), 30);

        assert!(presenter.step(&flights, &display), "aircraft appears");
        assert_eq!(lines(&presenter), ["EZY32GY", "1200ft 153°"]);
        assert_eq!(presenter.shown(&flights), Some(0));
        assert_eq!(
            presenter.frame(&display),
            display.scene(&lines(&presenter)).frame(0)
        );

        assert_eq!(
            until_change(&mut presenter, &flights, &display),
            30,
            "3 s at 10 ticks/s"
        );
        assert_eq!(lines(&presenter), ["EZY32GY", "A320 G-EZTB"]);
        assert_eq!(until_change(&mut presenter, &flights, &display), 30);
        assert_eq!(
            lines(&presenter),
            ["EZY32GY", "1200ft 153°"],
            "back to page one"
        );
    }

    #[test]
    fn page_without_values_is_skipped() {
        // Not in the aircraft database: the type page has nothing to say.
        let display = display(&[]);
        let mut unknown = anonymous(0xabc123);
        unknown.altitude_ft = Some(900);
        let flights = [unknown];
        let mut presenter = Presenter::new(default_templates(), 30);
        presenter.step(&flights, &display);
        assert_eq!(lines(&presenter), ["ABC123", "900ft"]);
        for _ in 0..200 {
            presenter.step(&flights, &display);
            assert_eq!(lines(&presenter), ["ABC123", "900ft"]);
        }
    }

    #[test]
    fn a_page_is_shown_even_if_no_page_is_complete() {
        let display = display(&[]);
        let flights = [anonymous(0xabc123)];
        let mut presenter =
            Presenter::new(templates(&["{{ callsign }}", "{{ aircraft_code }}"]), 30);
        presenter.step(&flights, &display);
        assert_eq!(lines(&presenter), [""], "first template, although empty");
        assert_eq!(presenter.shown(&flights), Some(0));
    }

    #[test]
    fn pages_stay_with_one_aircraft_while_another_waits() {
        let display = display(&[]);
        let flights = [flight(1, "AAA"), flight(2, "BBB")];
        let mut presenter = Presenter::new(
            templates(&["{{ name }}", "{{ name }} {{ index }}/{{ count }}"]),
            20,
        );

        let mut seen = Vec::new();
        presenter.step(&flights, &display);
        for _ in 0..5 {
            seen.push(lines(&presenter)[0].clone());
            assert_eq!(until_change(&mut presenter, &flights, &display), 20);
            assert_eq!(presenter.shown(&flights), Some(0));
        }
        assert_eq!(seen, ["AAA", "AAA 1/2", "AAA", "AAA 1/2", "AAA"]);

        // Only when AAA has left the rectangle is it the turn of BBB.
        assert!(presenter.step(&flights[1..], &display));
        assert_eq!(lines(&presenter), ["BBB"]);
    }

    #[test]
    fn single_page_stays_up_without_a_change() {
        let display = display(&[]);
        let flights = [flight(1, "AAA"), flight(2, "BBB")];
        let mut presenter = Presenter::new(templates(&["{{ name }}"]), 20);
        assert!(presenter.step(&flights, &display));
        for _ in 0..100 {
            assert!(!presenter.step(&flights, &display));
            assert_eq!(lines(&presenter), ["AAA"]);
        }
    }

    #[test]
    fn static_page_shows_live_values() {
        let display = display(&[]);
        let mut flights = [easyjet()];
        let mut presenter = Presenter::new(templates(&["{{ altitude }}"]), 1000);
        presenter.step(&flights, &display);
        assert_eq!(lines(&presenter), ["1200ft"]);

        assert!(!presenter.step(&flights, &display), "nothing changed");
        flights[0].altitude_ft = Some(1175);
        assert!(
            presenter.step(&flights, &display),
            "new altitude shows at once"
        );
        assert_eq!(lines(&presenter), ["1175ft"]);
        assert_eq!(
            presenter.frame(&display),
            display.scene(&["1175ft"]).frame(0)
        );
    }

    #[test]
    fn wide_template_scrolls_across_the_display() {
        let display = display(&[]);
        let flights = [easyjet()];
        let source = "{{ callsign }} {{ aircraft code }} {{ track }} {{ altitude }}";
        let mut presenter = Presenter::new(templates(&[source]), 30);
        presenter.step(&flights, &display);

        let text = "EZY32GY A320 153° 1200ft";
        assert_eq!(lines(&presenter), [text]);
        let scene = display.scene(&[text]);
        assert!(scene.scrolls(), "24 characters do not fit 11");
        // 24 characters * 5 dots - 0 spacing = 120, plus a 10 dot gap, plus
        // a 1 s rest of 10 ticks.
        assert_eq!(scene.cycle_ticks(), Some(140));

        // Every tick shows the scene one dot further, wrapping after a pass.
        for tick in 1..=300u64 {
            presenter.step(&flights, &display);
            assert_eq!(
                presenter.frame(&display),
                scene.frame(tick % 140),
                "tick {tick}"
            );
        }
    }

    #[test]
    fn scrolling_page_keeps_its_values_until_the_pass_is_over() {
        let display = display(&[]);
        let mut flights = [easyjet()];
        let source = "{{ callsign }} {{ aircraft code }} {{ track }} {{ altitude }}";
        let mut presenter = Presenter::new(templates(&[source]), 30);
        presenter.step(&flights, &display);

        // The altitude changes right after the pass started.
        flights[0].altitude_ft = Some(1100);
        for tick in 1..140 {
            assert!(
                !presenter.step(&flights, &display),
                "tick {tick}: text must not jump"
            );
            assert_eq!(lines(&presenter), ["EZY32GY A320 153° 1200ft"]);
        }
        // The single page comes round again with the new value.
        assert!(presenter.step(&flights, &display));
        assert_eq!(lines(&presenter), ["EZY32GY A320 153° 1100ft"]);
        assert_eq!(
            presenter.page.as_ref().unwrap().tick,
            0,
            "new pass starts at the left"
        );
    }

    #[test]
    fn scrolling_page_finishes_its_pass_before_the_next_page() {
        let display = display(&[]);
        let flights = [flight(1, "AAA"), flight(2, "BBB")];
        let source = "{{ callsign }} {{ aircraft code }} {{ track }} {{ altitude }}";
        // 2 s per page is far shorter than one pass of 12 s.
        let mut presenter = Presenter::new(templates(&[source, "{{ name }}"]), 20);
        presenter.step(&flights, &display);
        assert_eq!(lines(&presenter), ["AAA A320 153° 1200ft"]);
        // 20 characters: 100 dots + 10 gap + 10 rest.
        assert_eq!(
            until_change(&mut presenter, &flights, &display),
            120,
            "one whole pass"
        );
        assert_eq!(lines(&presenter), ["AAA"]);
    }

    #[test]
    fn long_page_time_repeats_whole_passes() {
        let display = display(&[]);
        let flights = [flight(1, "AAA"), flight(2, "BBB")];
        let source = "{{ callsign }} {{ aircraft code }} {{ track }} {{ altitude }}";
        // 20 s per page, one pass is 12 s: two passes, 24 s.
        let mut presenter = Presenter::new(templates(&[source, "{{ name }}"]), 200);
        presenter.step(&flights, &display);
        assert_eq!(until_change(&mut presenter, &flights, &display), 240);
        assert_eq!(lines(&presenter), ["AAA"]);
    }

    #[test]
    fn scroll_always_moves_a_short_template() {
        let display = display(&["--scroll", "always"]);
        let flights = [easyjet()];
        let mut presenter = Presenter::new(templates(&["{{ aircraft code }} {{ track }}"]), 30);
        presenter.step(&flights, &display);
        assert_eq!(lines(&presenter), ["A320 153°"]);
        let start = presenter.frame(&display);
        presenter.step(&flights, &display);
        // Rest for 10 ticks, then move.
        for _ in 0..10 {
            presenter.step(&flights, &display);
        }
        assert_ne!(
            presenter.frame(&display),
            start,
            "9 characters fit, but scroll anyway"
        );
    }

    #[test]
    fn departed_aircraft_is_replaced_at_once() {
        let display = display(&[]);
        let all = [flight(1, "AAA"), flight(2, "BBB"), flight(3, "CCC")];
        let mut presenter = Presenter::new(templates(&["{{ name }}"]), 1000);
        presenter.step(&all, &display);
        assert_eq!(lines(&presenter), ["AAA"]);

        // CCC, which is waiting, leaves the rectangle: nothing changes.
        assert!(!presenter.step(&all[..2], &display));
        assert_eq!(lines(&presenter), ["AAA"]);

        // AAA leaves: BBB has waited longest. DDD entered in the meantime.
        let remaining = [all[1].clone(), flight(4, "DDD")];
        assert!(presenter.step(&remaining, &display));
        assert_eq!(lines(&presenter), ["BBB"]);
        assert_eq!(presenter.shown(&remaining), Some(0));

        // BBB leaves too: on to DDD.
        assert!(presenter.step(&remaining[1..], &display));
        assert_eq!(lines(&presenter), ["DDD"]);

        // The last one leaves: dark.
        assert!(presenter.step(&[], &display));
        assert_eq!(presenter.frame(&display), FrameBuffer::new(56, 14));
        assert!(!presenter.step(&[], &display), "already dark");
    }

    #[test]
    fn typical_default_pages_fit_without_scrolling() {
        // The longest callsign is 8 characters; below 5000 ft altitude and
        // track are at most 11, and so is a 4 letter type with a 6
        // character registration.
        let display = display(&[]);
        let mut f = flight(0x405f12, "EZY32GYX");
        f.altitude_ft = Some(4975);
        f.track_deg = Some(359.0);
        for template in default_templates() {
            let page = template.render(&f, 0, 1).lines;
            assert_eq!(page[1].chars().count(), 11, "{page:?}");
            assert!(!display.scene(&page).scrolls(), "{page:?}");
        }
    }

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
