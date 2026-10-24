//! What is on the display at every tick.
//!
//! Each aircraft inside the rectangle is turned into text by one or more
//! [`Template`]s, and the text is laid out as a [`Scene`].

use crate::domain::dots::FrameBuffer;
use crate::domain::filter::Flight;
use crate::domain::scene::{Scene, Typesetting};
use crate::domain::template::Template;

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
/// inside the longest comes up. Lines that stand still show live values.
/// A scrolling line keeps its values for one pass, because text that
/// changes under way would jump.
pub struct Presenter {
    templates: Vec<Template>,
    /// Minimum time a page stays up, in ticks.
    page_ticks: u64,
    page: Option<Page>,
}

impl Presenter {
    pub fn new(templates: Vec<Template>, page_ticks: u64) -> Self {
        Self {
            templates,
            page_ticks: page_ticks.max(1),
            page: None,
        }
    }

    /// The aircraft on the display, as its position in `flights`.
    pub fn shown(&self, flights: &[Flight]) -> Option<usize> {
        let icao = self.page.as_ref()?.icao;
        flights.iter().position(|f| f.icao == icao)
    }

    /// The dots to show now.
    pub fn frame(&self, typesetting: &Typesetting) -> FrameBuffer {
        match &self.page {
            Some(page) => page.scene.frame(page.tick),
            None => typesetting.blank(),
        }
    }

    /// Bring up the first page of aircraft `aircraft` worth showing, trying
    /// template `template` first and then every other one. If no page has
    /// anything to say, the first template is shown regardless.
    fn open(
        &mut self,
        flights: &[Flight],
        typesetting: &Typesetting,
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
            scene: typesetting.scene(&lines),
            lines,
            elapsed: 0,
            tick: 0,
        });
    }

    /// Advance one tick. Returns true when the text on the display changed,
    /// as opposed to a scrolling line merely moving on.
    pub fn step(&mut self, flights: &[Flight], typesetting: &Typesetting) -> bool {
        if flights.is_empty() {
            return self.page.take().is_some();
        }
        let Some(page) = &mut self.page else {
            self.open(flights, typesetting, 0, 0);
            return true;
        };
        let Some(index) = flights.iter().position(|f| f.icao == page.icao) else {
            // The aircraft left: on to the one that has waited longest.
            self.open(flights, typesetting, 0, 0);
            return true;
        };

        page.elapsed += 1;
        page.tick += 1;
        // A scrolling page may only change between two passes.
        let cycle = page.scene.cycle_ticks();
        if cycle.is_some_and(|cycle| page.tick % cycle != 0) {
            // Under way, only the lines that stand still take new values.
            let fresh = self.templates[page.template]
                .render(&flights[index], index, flights.len())
                .lines;
            let lines: Vec<String> = fresh
                .into_iter()
                .zip(&page.lines)
                .enumerate()
                .map(|(i, (new, old))| {
                    if page.scene.line_scrolls(i) {
                        old.clone()
                    } else {
                        new
                    }
                })
                .collect();
            if lines == page.lines {
                return false;
            }
            let scene = typesetting.scene(&lines);
            // A line that starts or stops scrolling waits for the pass.
            if !scene.scrolls_like(&page.scene) {
                return false;
            }
            page.scene = scene;
            page.lines = lines;
            return true;
        }
        if page.elapsed >= self.page_ticks {
            let (template, lines) = (page.template + 1, page.lines.clone());
            self.open(flights, typesetting, index, template);
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
        page.scene = typesetting.scene(&lines);
        page.lines = lines;
        page.tick = 0;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::scene::{Align, Font, Scroll};
    use crate::domain::template::templates as standard;
    use crate::domain::template::tests::{anonymous, easyjet};

    /// Two 28x14 panels, 5x7 font, 10 dots per second: 11 characters fit.
    fn display(scroll: Scroll) -> Typesetting {
        Typesetting {
            width: 56,
            height: 14,
            font: Font::F5x7,
            align: Align::Left,
            scroll,
            hold: 10,
        }
    }

    /// Two pages that alternate: where to, and what it is.
    const TWO_PAGES: [&str; 2] = [
        "{{ name }}\\n{{ altitude }} {{ track }}",
        "{{ name }}\\n{{ aircraft_code }} {{ registration }}",
    ];

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
    fn until_change(presenter: &mut Presenter, flights: &[Flight], display: &Typesetting) -> u64 {
        for ticks in 1..10_000 {
            if presenter.step(flights, display) {
                return ticks;
            }
        }
        panic!("text never changed");
    }

    #[test]
    fn display_is_dark_without_aircraft() {
        let display = display(Scroll::Auto);
        let mut presenter = Presenter::new(standard(), 30);
        assert!(!presenter.step(&[], &display));
        assert_eq!(presenter.frame(&display), FrameBuffer::new(56, 14));
        assert_eq!(presenter.shown(&[]), None);
    }

    #[test]
    fn pages_alternate_for_one_aircraft() {
        let display = display(Scroll::Auto);
        let flights = [easyjet()];
        let mut presenter = Presenter::new(templates(&TWO_PAGES), 30);

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
        let display = display(Scroll::Auto);
        let mut unknown = anonymous(0xabc123);
        unknown.altitude_ft = Some(900);
        let flights = [unknown];
        let mut presenter = Presenter::new(templates(&TWO_PAGES), 30);
        presenter.step(&flights, &display);
        assert_eq!(lines(&presenter), ["ABC123", "900ft"]);
        for _ in 0..200 {
            presenter.step(&flights, &display);
            assert_eq!(lines(&presenter), ["ABC123", "900ft"]);
        }
    }

    #[test]
    fn a_page_is_shown_even_if_no_page_is_complete() {
        let display = display(Scroll::Auto);
        let flights = [anonymous(0xabc123)];
        let mut presenter =
            Presenter::new(templates(&["{{ callsign }}", "{{ aircraft_code }}"]), 30);
        presenter.step(&flights, &display);
        assert_eq!(lines(&presenter), [""], "first template, although empty");
        assert_eq!(presenter.shown(&flights), Some(0));
    }

    #[test]
    fn pages_stay_with_one_aircraft_while_another_waits() {
        let display = display(Scroll::Auto);
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
        let display = display(Scroll::Auto);
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
        let display = display(Scroll::Auto);
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
        let display = display(Scroll::Auto);
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
        let display = display(Scroll::Auto);
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
    fn line_that_stands_still_updates_while_the_other_scrolls() {
        // Three panels: 16 characters. The first line has 17.
        let display = Typesetting {
            width: 84,
            ..display(Scroll::Auto)
        };
        let mut flights = [easyjet()];
        let source =
            "{{ callsign }} {{ aircraft code }} {{ track }}\\n{{ speed }}{{ fill }}{{ altitude }}";
        let mut presenter = Presenter::new(templates(&[source]), 1000);
        presenter.step(&flights, &display);
        let scene = display.scene(&lines(&presenter));
        let cycle = scene.cycle_ticks().unwrap();
        for _ in 0..20 {
            presenter.step(&flights, &display);
        }

        // Under way, the altitude changes and the track too.
        flights[0].altitude_ft = Some(1100);
        flights[0].track_deg = Some(170.0);
        assert!(presenter.step(&flights, &display), "new altitude at once");
        assert_eq!(lines(&presenter), ["EZY32GY A320 153°", "151kt\t1100ft"]);
        let moved = display.scene(&["EZY32GY A320 153°", "151kt\t1100ft"]);
        assert_eq!(
            presenter.frame(&display),
            moved.frame(21),
            "the scrolling line keeps its place"
        );

        // The track only changes when the pass is over.
        for _ in 22..cycle {
            assert!(!presenter.step(&flights, &display));
        }
        assert!(presenter.step(&flights, &display));
        assert_eq!(lines(&presenter), ["EZY32GY A320 170°", "151kt\t1100ft"]);
    }

    #[test]
    fn speed_and_altitude_scroll_where_they_do_not_fit() {
        // One panel: 5 characters, and `151kt 1200ft` has 12.
        let display = Typesetting {
            width: 28,
            ..display(Scroll::Auto)
        };
        let mut flights = [easyjet()];
        let mut presenter = Presenter::new(standard(), 1000);
        presenter.step(&flights, &display);
        let scene = display.scene(&lines(&presenter));
        assert!(scene.line_scrolls(1), "the second line scrolls");
        assert_eq!(
            scene,
            display.scene(&[&lines(&presenter)[0], "151kt 1200ft"])
        );

        // Its values are kept for the pass, like any scrolling line.
        flights[0].altitude_ft = Some(1100);
        assert!(!presenter.step(&flights, &display));
        assert_eq!(lines(&presenter)[1], "151kt\t1200ft");
    }

    #[test]
    fn scrolling_page_finishes_its_pass_before_the_next_page() {
        let display = display(Scroll::Auto);
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
        let display = display(Scroll::Auto);
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
        let display = display(Scroll::Always);
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
        let display = display(Scroll::Auto);
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
    fn default_page_sets_speed_and_altitude_at_the_edges() {
        // Four panels of 28 dots: 22 characters, enough for both.
        let display = Typesetting {
            width: 112,
            ..display(Scroll::Auto)
        };
        let flights = [easyjet()];
        let mut presenter = Presenter::new(standard(), 30);
        presenter.step(&flights, &display);
        assert_eq!(lines(&presenter), ["flight EZY32GY", "151kt\t1200ft"]);
        let scene = display.scene(&lines(&presenter));
        assert!(!scene.scrolls());

        // The second line is speed on the left and altitude on the right.
        let side = |text, align| Typesetting { align, ..display }.scene(&["", text]).frame(0);
        let (speed, altitude) = (side("151kt", Align::Left), side("1200ft", Align::Right));
        let frame = scene.frame(0);
        for y in 7..14 {
            for x in 0..112 {
                let expected = speed.get(x, y) || altitude.get(x, y);
                assert_eq!(frame.get(x, y), expected, "dot {x},{y}");
            }
        }
    }

    #[test]
    fn default_pages_show_name_registration_and_type_in_turn() {
        let display = display(Scroll::Auto);
        let flights = [easyjet()];
        let mut presenter = Presenter::new(standard(), 30);
        presenter.step(&flights, &display);
        assert_eq!(lines(&presenter), ["flight EZY32GY", "151kt\t1200ft"]);
        // `flight EZY32GY` is too wide for 11 characters: it scrolls one
        // whole pass before the registration comes up.
        let pass = display.scene(&lines(&presenter)).cycle_ticks().unwrap();
        assert!(pass > 30, "{pass}");
        assert_eq!(until_change(&mut presenter, &flights, &display), pass);
        assert_eq!(lines(&presenter), ["reg G-EZTB", "151kt\t1200ft"]);

        // Only the top line changes, so only the top half flips.
        let before = display.scene(&["flight EZY32GY", "151kt\t1200ft"]).frame(0);
        let after = presenter.frame(&display);
        for y in 7..14 {
            for x in 0..56 {
                assert_eq!(after.get(x, y), before.get(x, y), "dot {x},{y}");
            }
        }

        // `AIRBUS A-320 (A320)` is too wide for 11 characters: it scrolls
        // one whole pass before the name comes back.
        assert_eq!(until_change(&mut presenter, &flights, &display), 30);
        assert_eq!(lines(&presenter), ["AIRBUS A-320 (A320)", "151kt\t1200ft"]);
        let pass = display.scene(&lines(&presenter)).cycle_ticks().unwrap();
        assert!(pass > 30, "{pass}");
        assert_eq!(until_change(&mut presenter, &flights, &display), pass);
        assert_eq!(lines(&presenter), ["flight EZY32GY", "151kt\t1200ft"]);
    }

    #[test]
    fn default_pages_of_an_unknown_aircraft_stay_on_its_address() {
        let display = display(Scroll::Auto);
        let flights = [anonymous(0xabc123)];
        let mut presenter = Presenter::new(standard(), 30);
        presenter.step(&flights, &display);
        assert_eq!(lines(&presenter), ["flight ABC123", "\t"]);
        for _ in 0..100 {
            assert!(!presenter.step(&flights, &display));
        }
    }
}
