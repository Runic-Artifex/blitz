//! Animation and transition events against Chromium 153 (measured on
//! scratch pages, 2026-10-14; the runtime's conformance cases compare whole
//! programs): phases and iterations sampled at each frame, elapsed times,
//! cancellations queued at the style update that makes them, and order.

use crate::qual_name;
use crate::{AnimationEvent, AnimationEventKind, Attribute, BaseDocument, DocumentConfig, NodeId};
use AnimationEventKind::*;
use blitz_traits::shell::{ColorScheme, Viewport};

const CSS: &str = "
@keyframes k { from { opacity: 0 } to { opacity: 1 } }
@keyframes k2 { from { translate: 0 } to { translate: 10px } }
div { width: 20px; height: 20px }
.t { transition: opacity 100ms linear }
.td { transition: opacity 100ms linear 50ms }
.hidden { opacity: 0 }
.pse::before, .pse::after { content: 'x'; animation: k 60ms linear }
";

/// A document with one div per style (in tree and creation order) under
/// a body, resolved at time 0 and sampled once.
fn doc(styles: &[(&str, &str)]) -> (BaseDocument, Vec<NodeId>) {
    let mut doc = BaseDocument::new(DocumentConfig {
        viewport: Some(Viewport::new(400, 400, 1.0, ColorScheme::Light)),
        ..Default::default()
    });
    doc.add_user_agent_stylesheet(CSS);
    doc.track_animation_events(true);
    let root = doc.root_node().id;
    let mut mutator = doc.mutate();
    let ids: Vec<NodeId> = styles
        .iter()
        .map(|(class, style)| {
            mutator.create_element(
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
            )
        })
        .collect();
    let html = mutator.create_element(qual_name!("html", html), vec![]);
    let body = mutator.create_element(qual_name!("body", html), vec![]);
    mutator.append_children(body, &ids);
    mutator.append_children(html, &[body]);
    mutator.append_children(root, &[html]);
    drop(mutator);
    (doc, ids)
}

fn set(doc: &mut BaseDocument, id: NodeId, name: &str, value: &str) {
    let name = match name {
        "class" => qual_name!("class"),
        _ => qual_name!("style"),
    };
    doc.mutate().set_attribute(id, name, value);
}

/// A frame at `ms`: the style update, then the sample.
fn frame(doc: &mut BaseDocument, ms: f64) -> Vec<(AnimationEventKind, NodeId, String, f64)> {
    doc.resolve(ms / 1000.0);
    sample(doc, ms)
}

fn sample(doc: &mut BaseDocument, ms: f64) -> Vec<(AnimationEventKind, NodeId, String, f64)> {
    doc.take_animation_events(ms / 1000.0)
        .into_iter()
        .map(|e: AnimationEvent| {
            let name = if e.pseudo_element.is_empty() {
                e.name
            } else {
                format!("{}{}", e.name, e.pseudo_element)
            };
            (e.kind, e.node, name, (e.elapsed_time * 1e6).round() / 1e6)
        })
        .collect()
}

fn ev(
    kind: AnimationEventKind,
    node: NodeId,
    name: &str,
    elapsed: f64,
) -> (AnimationEventKind, NodeId, String, f64) {
    (kind, node, name.to_string(), elapsed)
}

#[test]
fn start_iteration_end_and_delays() {
    let (mut d, ids) = doc(&[
        ("", "animation: k 100ms linear 2"),
        ("", "animation: k 100ms linear 50ms"),
        ("", "animation: k 100ms linear -40ms 2"),
        ("", "animation: k 100ms linear -250ms 2"),
    ]);
    let [a, b, c, e] = ids[..] else {
        unreachable!()
    };
    assert_eq!(
        frame(&mut d, 0.0),
        vec![
            ev(AnimationStart, a, "k", 0.0),
            // A negative delay starts past the start; one past the end
            // starts and ends at once, with the active duration.
            ev(AnimationStart, c, "k", 0.04),
            ev(AnimationStart, e, "k", 0.2),
            ev(AnimationEnd, e, "k", 0.2),
        ]
    );
    assert_eq!(frame(&mut d, 16.0), vec![]);
    assert_eq!(
        frame(&mut d, 64.0),
        vec![
            ev(AnimationStart, b, "k", 0.0),
            ev(AnimationIteration, c, "k", 0.1)
        ]
    );
    assert_eq!(
        frame(&mut d, 112.0),
        vec![ev(AnimationIteration, a, "k", 0.1)]
    );
    assert_eq!(
        frame(&mut d, 160.0),
        vec![ev(AnimationEnd, b, "k", 0.1), ev(AnimationEnd, c, "k", 0.2)]
    );
    assert_eq!(frame(&mut d, 208.0), vec![ev(AnimationEnd, a, "k", 0.2)]);
    assert_eq!(frame(&mut d, 400.0), vec![]);
}

#[test]
fn skipped_iterations_report_the_current_boundary() {
    // Chromium 153: one event per frame, `elapsedTime` the boundary of the
    // current iteration (5 ms iterations at 16 ms frames: 0.015, 0.03, ...).
    let (mut d, ids) = doc(&[("", "animation: k 5ms linear 20")]);
    let a = ids[0];
    assert_eq!(frame(&mut d, 0.0), vec![ev(AnimationStart, a, "k", 0.0)]);
    assert_eq!(
        frame(&mut d, 16.0),
        vec![ev(AnimationIteration, a, "k", 0.015)]
    );
    assert_eq!(
        frame(&mut d, 32.0),
        vec![ev(AnimationIteration, a, "k", 0.03)]
    );
    assert_eq!(frame(&mut d, 112.0), vec![ev(AnimationEnd, a, "k", 0.1)]);
}

#[test]
fn cancellations_are_queued_for_the_next_frame() {
    let (mut d, ids) = doc(&[
        ("", "animation: k 200ms linear"),
        ("", "animation: k 200ms linear 100ms"),
        ("", "animation: k 50ms linear forwards"),
    ]);
    let [a, b, c] = ids[..] else { unreachable!() };
    assert_eq!(
        frame(&mut d, 0.0),
        vec![
            ev(AnimationStart, a, "k", 0.0),
            ev(AnimationStart, c, "k", 0.0)
        ]
    );
    assert_eq!(frame(&mut d, 64.0), vec![ev(AnimationEnd, c, "k", 0.05)]);
    // A style update cancels at its time: the animation's current time
    // (delay included); a finished animation fires none.
    set(&mut d, a, "style", "animation: none");
    set(&mut d, b, "style", "animation: none");
    set(&mut d, c, "style", "animation: none");
    d.resolve(0.07);
    assert_eq!(
        sample(&mut d, 80.0),
        vec![
            ev(AnimationCancel, a, "k", 0.07),
            ev(AnimationCancel, b, "k", 0.07)
        ]
    );
    assert_eq!(frame(&mut d, 96.0), vec![]);
}

#[test]
fn an_animation_cancelled_before_any_frame_fires_nothing() {
    let (mut d, ids) = doc(&[("", "animation: k 100ms linear")]);
    d.resolve(0.0);
    set(&mut d, ids[0], "style", "animation: none");
    assert_eq!(frame(&mut d, 0.0), vec![]);
}

#[test]
fn renaming_cancels_then_starts() {
    let (mut d, ids) = doc(&[("", "animation: k 200ms linear")]);
    let a = ids[0];
    frame(&mut d, 0.0);
    set(&mut d, a, "style", "animation: k2 200ms linear");
    assert_eq!(
        frame(&mut d, 64.0),
        vec![
            ev(AnimationCancel, a, "k", 0.064),
            ev(AnimationStart, a, "k2", 0.0)
        ]
    );
}

#[test]
fn display_none_on_an_ancestor_and_removal_cancel() {
    let (mut d, ids) = doc(&[("", ""), ("", "animation: k 100ms linear")]);
    let [parent, child] = ids[..] else {
        unreachable!()
    };
    // Move the animated div into the first one.
    {
        let mut m = d.mutate();
        m.remove_node(child);
        m.append_children(parent, &[child]);
    }
    assert_eq!(
        frame(&mut d, 0.0),
        vec![ev(AnimationStart, child, "k", 0.0)]
    );
    set(&mut d, parent, "style", "display: none");
    d.resolve(0.05);
    assert_eq!(
        sample(&mut d, 64.0),
        vec![ev(AnimationCancel, child, "k", 0.05)]
    );
    // Shown again: a new animation from its start.
    set(&mut d, parent, "style", "");
    assert_eq!(
        frame(&mut d, 80.0),
        vec![ev(AnimationStart, child, "k", 0.0)]
    );
    d.mutate().remove_node(parent);
    d.resolve(0.1);
    assert_eq!(
        sample(&mut d, 112.0),
        vec![ev(AnimationCancel, child, "k", 0.02)]
    );
}

#[test]
fn transitions_run_start_end_delay_and_reverse() {
    let (mut d, ids) = doc(&[("t hidden", ""), ("td hidden", "")]);
    let [a, b] = ids[..] else { unreachable!() };
    frame(&mut d, 0.0);
    set(&mut d, a, "class", "t");
    set(&mut d, b, "class", "td");
    assert_eq!(
        frame(&mut d, 16.0),
        vec![
            ev(TransitionRun, a, "opacity", 0.0),
            ev(TransitionStart, a, "opacity", 0.0),
            ev(TransitionRun, b, "opacity", 0.0),
        ]
    );
    assert_eq!(frame(&mut d, 64.0), vec![]);
    assert_eq!(
        frame(&mut d, 80.0),
        vec![ev(TransitionStart, b, "opacity", 0.0)]
    );
    // Reversed half way: cancelled with its active time, then a shortened
    // transition back.
    set(&mut d, a, "class", "t hidden");
    let events = frame(&mut d, 66.0 + 16.0);
    assert_eq!(events[0], ev(TransitionCancel, a, "opacity", 0.066));
    assert_eq!(
        &events[1..],
        &[
            ev(TransitionRun, a, "opacity", 0.0),
            ev(TransitionStart, a, "opacity", 0.0)
        ]
    );
    // Ends: the reversed one after its shortened duration.
    assert_eq!(
        frame(&mut d, 160.0),
        vec![ev(TransitionEnd, a, "opacity", 0.066)]
    );
    // The delayed one: created at 16 ms, started at 66 ms.
    assert_eq!(
        frame(&mut d, 176.0),
        vec![ev(TransitionEnd, b, "opacity", 0.1)]
    );
}

#[test]
fn a_transition_ending_between_frames_still_ends_at_the_next_frame() {
    let (mut d, ids) = doc(&[("t hidden", "")]);
    let a = ids[0];
    frame(&mut d, 0.0);
    set(&mut d, a, "class", "t");
    frame(&mut d, 16.0);
    // A style update after the end drops the transition before the frame.
    d.resolve(0.13);
    assert_eq!(
        sample(&mut d, 128.0),
        vec![ev(TransitionEnd, a, "opacity", 0.1)]
    );
}

#[test]
fn order_transitions_first_then_creation_order_and_pseudo_elements() {
    let (mut d, ids) = doc(&[
        ("", ""),
        ("t hidden", ""),
        ("pse", "animation: k2 60ms linear"),
    ]);
    let [a, b, c] = ids[..] else { unreachable!() };
    // An element before its ::before and ::after.
    assert_eq!(
        frame(&mut d, 0.0),
        vec![
            ev(AnimationStart, c, "k2", 0.0),
            ev(AnimationStart, c, "k::before", 0.0),
            ev(AnimationStart, c, "k::after", 0.0),
        ]
    );
    set(
        &mut d,
        a,
        "style",
        "animation: k 60ms linear, k2 60ms linear",
    );
    set(&mut d, b, "class", "t");
    // Transitions before animations; an element's animations in the order
    // of animation-name.
    assert_eq!(
        frame(&mut d, 16.0),
        vec![
            ev(TransitionRun, b, "opacity", 0.0),
            ev(TransitionStart, b, "opacity", 0.0),
            ev(AnimationStart, a, "k", 0.0),
            ev(AnimationStart, a, "k2", 0.0),
        ]
    );
}

#[test]
fn transitions_started_together_are_marked_unordered() {
    let (mut d, ids) = doc(&[("", "transition: width 100ms linear, opacity 100ms linear")]);
    let a = ids[0];
    frame(&mut d, 0.0);
    set(
        &mut d,
        a,
        "style",
        "transition: width 100ms linear, opacity 100ms linear; width: 30px; opacity: 0.5",
    );
    let events = d.take_animation_events(0.0);
    assert!(events.is_empty());
    d.resolve(0.016);
    let events = d.take_animation_events(0.016);
    let kinds: Vec<_> = events
        .iter()
        .map(|e| (e.kind, e.name.as_str(), e.unordered))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (TransitionRun, "opacity", true),
            (TransitionStart, "opacity", true),
            (TransitionRun, "width", true),
            (TransitionStart, "width", true),
        ]
    );
    // Their ends are ordered by property name.
    d.resolve(0.116);
    let ends: Vec<_> = d
        .take_animation_events(0.116)
        .into_iter()
        .map(|e| (e.name, e.unordered))
        .collect();
    assert_eq!(
        ends,
        vec![("opacity".to_string(), false), ("width".to_string(), false)]
    );
}
