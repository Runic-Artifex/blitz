//! Scroll-driven animations against Chromium 153 (measured on scratch pages,
//! 2026-10-14): a 200x100 `overflow: auto` scroller with 600x500 content
//! (maximum scroll offset 400 in both axes), 1ms linear `both` animations
//! unless stated. Every value is checked after a single resolve at time 0:
//! no time passes, the scroll offset and layout drive the animations.

use crate::qual_name;
use crate::{
    Attribute, BaseDocument, DocumentConfig, NodeId, ScrollBehavior, ScrollDrivenAnimation,
    UnsupportedScrollDriven,
};
use blitz_traits::shell::{ColorScheme, Viewport};
use style::animation::UnsupportedTimeline;

const CSS: &str = "
html, body { margin: 0 }
@keyframes p { from { opacity: 0 } to { opacity: 1 } }
@keyframes q { from { opacity: 0.2 } to { opacity: 0.8 } }
@keyframes t { from { --t: 0px } to { --t: 40px } }
@property --t { syntax: '<length-percentage>'; inherits: false; initial-value: 0px }
.s { width: 200px; height: 100px; overflow: auto }
.pse::before {
  content: ''; display: block; height: 10px;
  animation: p 1ms linear both; animation-timeline: scroll(nearest)
}
";

/// An element: its inline style (or `.class; style`) and children.
struct El(&'static str, Vec<El>);

fn el(style: &'static str) -> El {
    El(style, vec![])
}

/// A scroller (with `class="s"`) whose first child is the 600x500 content.
fn scroller(style: &'static str, mut children: Vec<El>) -> El {
    children.insert(0, El("width: 600px; height: 500px", vec![]));
    El(style, children)
}

/// A document whose body holds `body`, in a viewport of `width`x`height`,
/// resolved at time 0. Returns the elements in tree order (the body's
/// children and their descendants).
fn doc_in(width: u32, height: u32, body: Vec<El>) -> (BaseDocument, Vec<NodeId>) {
    let mut doc = BaseDocument::new(DocumentConfig {
        viewport: Some(Viewport::new(width, height, 1.0, ColorScheme::Light)),
        ..Default::default()
    });
    doc.add_user_agent_stylesheet(CSS);
    doc.track_animation_events(true);
    let root = doc.root_node().id;
    let mut ids = Vec::new();
    {
        let mut mutator = doc.mutate();
        fn build(
            mutator: &mut crate::DocumentMutator<'_>,
            el: &El,
            ids: &mut Vec<NodeId>,
        ) -> NodeId {
            let (class, style) = match el.0.strip_prefix('.') {
                Some(rest) => rest.split_once(';').unwrap_or((rest, "")),
                None => ("", el.0),
            };
            let id = mutator.create_element(
                qual_name!("div", html),
                vec![
                    Attribute {
                        name: qual_name!("class"),
                        value: class.to_string(),
                    },
                    Attribute {
                        name: qual_name!("style"),
                        value: style.to_string(),
                    },
                ],
            );
            ids.push(id);
            let children: Vec<NodeId> = el.1.iter().map(|c| build(mutator, c, ids)).collect();
            mutator.append_children(id, &children);
            id
        }
        let children: Vec<NodeId> = body
            .iter()
            .map(|e| build(&mut mutator, e, &mut ids))
            .collect();
        let html = mutator.create_element(qual_name!("html", html), vec![]);
        let body = mutator.create_element(qual_name!("body", html), vec![]);
        mutator.append_children(body, &children);
        mutator.append_children(html, &[body]);
        mutator.append_children(root, &[html]);
    }
    doc.resolve(0.0);
    (doc, ids)
}

fn doc(body: Vec<El>) -> (BaseDocument, Vec<NodeId>) {
    doc_in(400, 400, body)
}

fn opacity(doc: &BaseDocument, id: NodeId) -> f64 {
    doc.get_node(id)
        .unwrap()
        .primary_styles()
        .unwrap()
        .get_effects()
        .opacity as f64
}

/// Scroll `scroller` to `(x, y)` and resolve, without time passing.
fn scroll(doc: &mut BaseDocument, scroller: NodeId, x: f64, y: f64) {
    doc.scroll_to(scroller, x, y, ScrollBehavior::Instant);
    doc.resolve(0.0);
}

#[track_caller]
fn assert_close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} is not {expected} (± {tolerance})"
    );
}

/// The opacity of `animated` at each vertical offset of `scroller`.
#[track_caller]
fn check_y(doc: &mut BaseDocument, scroller: NodeId, animated: NodeId, cases: &[(f64, f64)]) {
    for &(offset, expected) in cases {
        scroll(doc, scroller, 0.0, offset);
        let actual = opacity(doc, animated);
        assert!(
            (actual - expected).abs() <= 1e-6,
            "at offset {offset}: opacity {actual}, expected {expected}"
        );
    }
}

/// A scroller animating itself with `animation`, on `scroll(self)` unless
/// the style sets the timeline.
fn self_driven(style: &'static str) -> (BaseDocument, NodeId) {
    let (doc, ids) = doc(vec![scroller(style, vec![])]);
    (doc, ids[0])
}

const S: &str = "width: 200px; height: 100px; overflow: auto;";

#[test]
fn progress_is_the_offset_through_the_scroll_range() {
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; \
         animation: p 1ms linear both; animation-timeline: scroll(self)",
    );
    check_y(
        &mut d,
        s,
        s,
        &[
            (0.0, 0.0),
            (50.0, 0.125),
            (96.0, 0.24),
            (400.0, 1.0),
            (50.0, 0.125),
        ],
    );
}

#[test]
fn ranges_of_lengths_percentages_and_calc() {
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms linear both; \
         animation-timeline: scroll(self); animation-range: 0 96px",
    );
    check_y(
        &mut d,
        s,
        s,
        &[(50.0, 0.520833), (96.0, 1.0), (200.0, 1.0), (0.0, 0.0)],
    );

    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms linear both; \
         animation-timeline: scroll(self); animation-range: calc(100% - 96px) 100%",
    );
    check_y(
        &mut d,
        s,
        s,
        &[(100.0, 0.0), (304.0, 0.0), (350.0, 0.479167), (400.0, 1.0)],
    );

    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms linear both; \
         animation-timeline: scroll(self); animation-range: 25% 75%",
    );
    check_y(&mut d, s, s, &[(200.0, 0.5), (300.0, 1.0), (350.0, 1.0)]);

    // A range that ends before it starts is empty, at its start.
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms linear both; \
         animation-timeline: scroll(self); animation-range: 300px 100px",
    );
    check_y(&mut d, s, s, &[(0.0, 0.0), (200.0, 0.0), (400.0, 1.0)]);

    // A range beyond the scroll range.
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms linear both; \
         animation-timeline: scroll(self); animation-range: -100px 600px",
    );
    check_y(&mut d, s, s, &[(0.0, 0.142857), (400.0, 0.714286)]);
}

#[test]
fn time_durations_become_proportions_of_the_range() {
    // Delay 1s, duration 2s: the delay is the first third of the range.
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: p 2s linear 1s both; \
         animation-timeline: scroll(self)",
    );
    check_y(
        &mut d,
        s,
        s,
        &[(100.0, 0.0), (200.0, 0.25), (304.0, 0.64), (350.0, 0.8125)],
    );

    // `auto` divides the range between the iterations.
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: p auto linear 2 both; \
         animation-timeline: scroll(self)",
    );
    check_y(
        &mut d,
        s,
        s,
        &[
            (50.0, 0.25),
            (96.0, 0.48),
            (200.0, 0.0),
            (304.0, 0.52),
            (400.0, 1.0),
        ],
    );
}

#[test]
fn direction_and_easing() {
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms linear reverse both; \
         animation-timeline: scroll(self)",
    );
    check_y(&mut d, s, s, &[(0.0, 1.0), (100.0, 0.75), (400.0, 0.0)]);

    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms ease-in-out both; \
         animation-timeline: scroll(self)",
    );
    check_y(
        &mut d,
        s,
        s,
        &[
            (50.0, 0.031114),
            (96.0, 0.118786),
            (200.0, 0.5),
            (304.0, 0.881214),
        ],
    );
}

#[test]
fn fill_none_ends_at_the_range_end_except_at_the_timeline_end() {
    // Without fill, outside the range there is no effect (opacity 1); the
    // end of the active interval is exclusive.
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: q 1ms linear; \
         animation-timeline: scroll(self); animation-range: 100px 300px",
    );
    check_y(
        &mut d,
        s,
        s,
        &[
            (50.0, 1.0),
            (100.0, 0.2),
            (299.0, 0.797),
            (300.0, 1.0),
            (400.0, 1.0),
        ],
    );

    // At the end of the timeline, the end applies.
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: q 1ms linear; \
         animation-timeline: scroll(self)",
    );
    check_y(&mut d, s, s, &[(399.0, 0.7985), (400.0, 0.8)]);
}

#[test]
fn inactive_timelines_have_no_effect_whatever_the_fill() {
    // Not a scroll container.
    let (d, s) = self_driven(
        "width: 200px; height: 100px; overflow: visible; animation: q 1ms linear both; \
         animation-timeline: scroll(self)",
    );
    assert_eq!(opacity(&d, s), 1.0);

    // A scroll container that cannot scroll in the axis.
    let (d, ids) = doc(vec![El(
        "width: 200px; height: 100px; overflow: auto; animation: q 1ms linear both; \
         animation-timeline: scroll(self)",
        vec![el("width: 100px; height: 50px")],
    )]);
    assert_eq!(opacity(&d, ids[0]), 1.0);
    let (d, ids) = doc(vec![El(
        "width: 200px; height: 100px; overflow: auto; animation: q 1ms linear both; \
         animation-timeline: scroll(self x)",
        vec![el("width: 100px; height: 500px")],
    )]);
    assert_eq!(opacity(&d, ids[0]), 1.0);

    // `overflow: hidden` is a scroll container (programmatic scrolls move it).
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: hidden; animation: p 1ms linear both; \
         animation-timeline: scroll(self)",
    );
    check_y(&mut d, s, s, &[(0.0, 0.0), (200.0, 0.5)]);
}

#[test]
fn nearest_and_root_scrollers() {
    // The nearest ancestor scroll container, past ancestors that are not;
    // `scroll()` is `scroll(nearest block)`.
    let (mut d, ids) = doc(vec![scroller(
        S,
        vec![
            el("animation: p 1ms linear both; animation-timeline: scroll(nearest)"),
            El(
                "",
                vec![el(
                    "animation: p 1ms linear both; animation-timeline: scroll()",
                )],
            ),
        ],
    )]);
    let (s, a, b) = (ids[0], ids[2], ids[4]);
    scroll(&mut d, s, 0.0, 100.0);
    assert_close(opacity(&d, a), 0.25, 1e-6);
    assert_close(opacity(&d, b), 0.25, 1e-6);

    // The element itself is excluded (the outer scroller's content is
    // 600px tall with it: maximum 500).
    let (mut d, ids) = doc(vec![scroller(
        S,
        vec![scroller(
            "width: 200px; height: 100px; overflow: auto; \
             animation: p 1ms linear both; animation-timeline: scroll(nearest)",
            vec![],
        )],
    )]);
    scroll(&mut d, ids[0], 0.0, 200.0);
    assert_close(opacity(&d, ids[2]), 0.4, 1e-6);

    // The root scroller: 2000px of content in a 300px viewport.
    let (mut d, ids) = doc_in(
        400,
        300,
        vec![
            el("height: 2000px"),
            el("animation: p 1ms linear both; animation-timeline: scroll(root)"),
            el("animation: p 1ms linear both; animation-timeline: scroll(nearest)"),
        ],
    );
    let root = d.root_element().id;
    d.scroll_to(root, 0.0, 850.0, ScrollBehavior::Instant);
    d.resolve(0.0);
    assert_close(opacity(&d, ids[1]), 0.5, 1e-6);
    assert_close(opacity(&d, ids[2]), 0.5, 1e-6);
}

#[test]
fn pseudo_elements_follow_their_own_scroller() {
    let (mut d, ids) = doc(vec![scroller(S, vec![el(".pse;")])]);
    let (s, host) = (ids[0], ids[2]);
    scroll(&mut d, s, 0.0, 100.0);
    let before = d.get_node(host).unwrap().before().unwrap();
    // The ::before box is 10px tall: the content is 510px, maximum 410.
    assert_close(opacity(&d, before), 100.0 / 410.0, 1e-6);
    assert_eq!(d.scroll_driven_animations()[0].pseudo_element, "::before");
}

#[test]
fn layout_changes_move_the_timeline() {
    let (mut d, ids) = doc(vec![El(
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms linear both; \
         animation-timeline: scroll(self)",
        vec![el("width: 600px; height: 500px")],
    )]);
    let (s, content) = (ids[0], ids[1]);
    scroll(&mut d, s, 0.0, 200.0);
    assert_close(opacity(&d, s), 0.5, 1e-6);
    d.mutate()
        .set_attribute(content, qual_name!("style"), "width: 600px; height: 900px");
    d.resolve(0.0);
    assert_close(opacity(&d, s), 0.25, 1e-6);
}

#[test]
fn horizontal_and_right_to_left_axes() {
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms linear both; \
         animation-timeline: scroll(self inline)",
    );
    scroll(&mut d, s, 100.0, 300.0);
    assert_close(opacity(&d, s), 0.25, 1e-6);

    // Right to left: the distance scrolled from the origin at the right.
    for timeline in ["scroll(self inline)", "scroll(self x)"] {
        let style: &'static str = if timeline.ends_with("inline)") {
            "direction: rtl; width: 200px; height: 100px; overflow: auto; \
             animation: p 1ms linear both; animation-timeline: scroll(self inline)"
        } else {
            "direction: rtl; width: 200px; height: 100px; overflow: auto; \
             animation: p 1ms linear both; animation-timeline: scroll(self x)"
        };
        let (mut d, s) = self_driven(style);
        assert_close(opacity(&d, s), 0.0, 1e-6);
        scroll(&mut d, s, -100.0, 0.0);
        assert_close(opacity(&d, s), 0.25, 1e-6);
        scroll(&mut d, s, -400.0, 0.0);
        assert_close(opacity(&d, s), 1.0, 1e-6);
    }
}

#[test]
fn a_changed_timeline_is_followed_in_place() {
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms linear both; \
         animation-timeline: scroll(self)",
    );
    scroll(&mut d, s, 100.0, 200.0);
    assert_close(opacity(&d, s), 0.5, 1e-6);
    d.mutate().set_attribute(
        s,
        qual_name!("style"),
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms linear both; \
         animation-timeline: scroll(self x)",
    );
    d.resolve(0.0);
    assert_close(opacity(&d, s), 0.25, 1e-6);
    d.mutate().set_attribute(
        s,
        qual_name!("style"),
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms linear both; \
         animation-timeline: scroll(self x); animation-range: 0 200px",
    );
    d.resolve(0.0);
    assert_close(opacity(&d, s), 0.5, 1e-6);
}

#[test]
fn registered_custom_properties_animate() {
    // Scroll-driven: ease-in-out over `0 96px`.
    let (mut d, ids) = doc(vec![scroller(
        "width: 200px; height: 100px; overflow: auto; animation: t 1ms ease-in-out both; \
         animation-timeline: scroll(self); animation-range: 0 96px",
        vec![el("width: var(--t)")],
    )]);
    let s = ids[0];
    scroll(&mut d, s, 0.0, 50.0);
    let value = d.resolved_style_value(s, "--t");
    let px: f64 = value.trim_end_matches("px").parse().unwrap();
    assert_close(px, 21.4352, 5e-5);
    scroll(&mut d, s, 0.0, 96.0);
    assert_eq!(d.resolved_style_value(s, "--t"), "40px");

    // Time-based, and a property using it through var().
    let (mut d, ids) = doc(vec![El(
        "animation: t 100ms linear both",
        vec![el("width: calc(var(--t) * 2); --t: inherit")],
    )]);
    d.resolve(0.025);
    d.resolve(0.025);
    let px: f64 = d
        .resolved_style_value(ids[0], "--t")
        .trim_end_matches("px")
        .parse()
        .unwrap();
    assert_close(px, 10.0, 1e-4);
    let width = d.get_node(ids[1]).unwrap().final_layout().size.width as f64;
    assert_close(width, 20.0, 1e-3);
}

#[test]
fn a_programmatic_scroll_updates_without_time_and_static_documents_are_idle() {
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: p 1s linear both; \
         animation-timeline: scroll(self)",
    );
    assert_eq!(opacity(&d, s), 0.0);
    // Only scroll-driven animations, nothing scrolling: idle.
    assert!(!d.is_animating());
    scroll(&mut d, s, 0.0, 300.0);
    assert_close(opacity(&d, s), 0.75, 1e-6);
    // No events for scroll-driven animations.
    assert!(d.take_animation_events(0.0).is_empty());
    assert!(d.take_animation_events(5.0).is_empty());

    // A time-based animation keeps the document animating.
    let (d, _) = doc(vec![el("animation: p 1s linear")]);
    assert!(d.is_animating());
}

#[test]
fn paused_from_the_start_holds_the_range_start() {
    let (mut d, s) = self_driven(
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms linear both paused; \
         animation-timeline: scroll(self)",
    );
    check_y(&mut d, s, s, &[(200.0, 0.0), (400.0, 0.0)]);
    assert_eq!(
        d.scroll_driven_animations(),
        vec![ScrollDrivenAnimation {
            node: s,
            pseudo_element: "",
            name: "p".into(),
            unsupported: None,
            paused: true,
        }]
    );

    // Resuming is not measured: no effect, and reported.
    scroll(&mut d, s, 0.0, 200.0);
    d.mutate().set_attribute(
        s,
        qual_name!("style"),
        "width: 200px; height: 100px; overflow: auto; animation: p 1ms linear both; \
         animation-timeline: scroll(self)",
    );
    d.resolve(0.0);
    assert_eq!(opacity(&d, s), 1.0);
    assert_eq!(
        d.scroll_driven_animations()[0].unsupported,
        Some(UnsupportedScrollDriven::Timeline(
            UnsupportedTimeline::PlayStateChange
        ))
    );
}

#[test]
fn unsupported_timelines_are_reported_and_have_no_effect() {
    let (d, ids) = doc(vec![
        el("animation: q 1ms linear both; animation-timeline: view()"),
        el("animation: q 1ms linear both; animation-timeline: --named"),
        scroller(
            "width: 200px; height: 100px; overflow: auto; animation: q 1ms linear both; \
             animation-timeline: scroll(self); animation-range: cover",
            vec![],
        ),
        scroller(
            "width: 200px; height: 100px; overflow: auto; animation: q 1ms linear infinite both; \
             animation-timeline: scroll(self)",
            vec![],
        ),
        scroller(
            "width: 200px; height: 100px; overflow: auto; animation: q 1s linear -1s both; \
             animation-timeline: scroll(self)",
            vec![],
        ),
        el("animation: q 1ms linear both; animation-timeline: none"),
    ]);
    let reported: Vec<_> = d
        .scroll_driven_animations()
        .into_iter()
        .map(|a| (a.node, a.unsupported))
        .collect();
    use UnsupportedScrollDriven::Timeline;
    use UnsupportedTimeline::*;
    assert_eq!(
        reported,
        vec![
            (ids[0], Some(Timeline(ViewTimeline))),
            (ids[1], Some(Timeline(NamedTimeline))),
            (ids[2], Some(Timeline(NamedRange))),
            (ids[4], Some(Timeline(Timing))),
            (ids[6], Some(Timeline(Timing))),
        ]
    );
    for id in [ids[0], ids[1], ids[2], ids[4], ids[6], ids[8]] {
        assert_eq!(opacity(&d, id), 1.0);
    }
}

#[test]
fn adding_an_animation_name_starts_it() {
    let (mut d, ids) = doc(vec![el("animation: p 1s linear both")]);
    let a = ids[0];
    d.mutate().set_attribute(
        a,
        qual_name!("style"),
        "animation: p 1s linear both, q 1s linear both; animation-timeline: auto, scroll()",
    );
    d.resolve(0.0);
    let names: Vec<_> = d
        .scroll_driven_animations()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, vec!["q".to_string()]);
}
