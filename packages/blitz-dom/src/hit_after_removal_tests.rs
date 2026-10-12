//! Hit testing between a DOM removal and the next resolve.
//!
//! The event driver runs default actions, which hit-test the pointer again,
//! after handlers that may have removed nodes, with no resolve in between (a
//! runtime closing a select's popup on the press that lands on the select).
//! The paint lists of the last resolve must not name the removed boxes then:
//! a dropped box would be dereferenced, and a detached one hit. Chromium
//! updates layout before every hit test, so a removed box is never hit.

use crate::{Attribute, BaseDocument, DocumentConfig, NodeId, qual_name};
use blitz_traits::shell::{ColorScheme, Viewport};

/// `(style, children)`
struct El(&'static str, Vec<El>);

/// Resolve `<html><body style="margin:0">` with the given divs in it; the
/// ids of the divs in document order.
fn resolve(divs: Vec<El>) -> (BaseDocument, Vec<NodeId>) {
    let mut doc = BaseDocument::new(DocumentConfig {
        viewport: Some(Viewport::new(400, 400, 1.0, ColorScheme::Light)),
        ..Default::default()
    });
    let root = doc.root_node().id;
    let mut ids = Vec::new();
    let mut mutator = doc.mutate();
    fn style(value: &str) -> Vec<Attribute> {
        vec![Attribute {
            name: qual_name!("style"),
            value: value.to_string(),
        }]
    }
    fn build(
        mutator: &mut crate::DocumentMutator<'_>,
        El(style_value, children): El,
        ids: &mut Vec<NodeId>,
    ) -> NodeId {
        let id = mutator.create_element(qual_name!("div", html), style(style_value));
        ids.push(id);
        let children: Vec<NodeId> = children
            .into_iter()
            .map(|child| build(mutator, child, ids))
            .collect();
        mutator.append_children(id, &children);
        id
    }
    let divs: Vec<NodeId> = divs
        .into_iter()
        .map(|div| build(&mut mutator, div, &mut ids))
        .collect();
    let html = mutator.create_element(qual_name!("html", html), vec![]);
    let body = mutator.create_element(qual_name!("body", html), style("margin:0"));
    mutator.append_children(body, &divs);
    mutator.append_children(html, &[body]);
    mutator.append_children(root, &[html]);
    drop(mutator);
    doc.resolve(0.0);
    (doc, ids)
}

fn el(style: &'static str) -> El {
    El(style, vec![])
}

fn hit(doc: &BaseDocument, x: f32, y: f32) -> Option<NodeId> {
    doc.hit(x, y).map(|hit| hit.node_id)
}

/// Points over the select, the popup and outside both.
const POINTS: [(f32, f32); 3] = [(50.0, 10.0), (50.0, 30.0), (300.0, 300.0)];

const SELECT: &str = "width:100px; height:20px";
/// A select's popup as the runtime draws it: fixed, under the select, with the
/// largest z-index (hoisted to the root element's stacking context).
const POPUP: &str =
    "position:fixed; left:0; top:20px; width:100px; height:60px; z-index:2147483647";

/// Hits at `POINTS` right after `remove` and after the next resolve. The
/// removed popup is out of flow, so the other boxes keep their layout and
/// both must be Chromium's hits: the popup is never hit.
fn hits_after(
    remove: impl Fn(&mut BaseDocument, NodeId),
) -> (Vec<Option<NodeId>>, Vec<Option<NodeId>>, Vec<NodeId>) {
    let (mut doc, ids) = resolve(vec![el(SELECT), El(POPUP, vec![el("height:20px")])]);
    assert_eq!(
        hit(&doc, 50.0, 30.0),
        Some(ids[2]),
        "the popup's item is hit"
    );
    remove(&mut doc, ids[1]);
    let before = POINTS.iter().map(|&(x, y)| hit(&doc, x, y)).collect();
    doc.resolve(0.0);
    let after = POINTS.iter().map(|&(x, y)| hit(&doc, x, y)).collect();
    (before, after, ids)
}

#[test]
fn dropped_hoisted_box_is_not_dereferenced_before_resolve() {
    let (before, after, ids) = hits_after(|doc, popup| {
        doc.mutate().remove_and_drop_node(popup);
    });
    assert_eq!(before[0], Some(ids[0]));
    assert_eq!(before, after);
}

#[test]
fn detached_hoisted_box_is_not_hit_before_resolve() {
    let (before, after, ids) = hits_after(|doc, popup| doc.mutate().remove_node(popup));
    assert_eq!(before[0], Some(ids[0]));
    assert!(!before.contains(&Some(ids[1])) && !before.contains(&Some(ids[2])));
    assert_eq!(before, after);
}

#[test]
fn box_moved_out_of_the_document_is_not_hit_before_resolve() {
    let (before, after, ids) = hits_after(|doc, popup| {
        let mut mutator = doc.mutate();
        let holder = mutator.create_element(qual_name!("div", html), vec![]);
        mutator.append_children(holder, &[popup]);
    });
    assert!(!before.contains(&Some(ids[1])) && !before.contains(&Some(ids[2])));
    assert_eq!(before, after);
}

/// A z-indexed box inside the removed subtree is hoisted to the root
/// element's stacking context, outside the subtree.
#[test]
fn box_hoisted_out_of_a_dropped_subtree_is_not_dereferenced() {
    let (mut doc, ids) = resolve(vec![
        el(SELECT),
        El(
            "height:0",
            vec![el(
                "position:relative; z-index:-1; width:100px; height:20px",
            )],
        ),
        el("position:relative; z-index:1; width:100px; height:20px"),
    ]);
    assert_eq!(hit(&doc, 50.0, 30.0), Some(ids[3]));
    let root = doc.root_element().id;
    let sc = |doc: &BaseDocument| {
        let sc = doc.nodes[root].stacking_context.as_ref().unwrap();
        let ids: Vec<NodeId> = sc.children.iter().map(|c| c.node_id).collect();
        (ids, sc.negative_z_count)
    };
    assert_eq!(sc(&doc), (vec![ids[2], ids[3]], 1));
    doc.mutate().remove_and_drop_node(ids[1]);
    assert_eq!(sc(&doc), (vec![ids[3]], 0));
    assert_eq!(hit(&doc, 50.0, 30.0), Some(ids[3]));
    assert_eq!(hit(&doc, 50.0, 10.0), Some(ids[0]));
    doc.resolve(0.0);
    assert_eq!(sc(&doc), (vec![ids[3]], 0));
    assert_eq!(hit(&doc, 50.0, 30.0), Some(ids[3]));
}

/// An in-flow box is listed in its parent's `paint_children`. The boxes after
/// it keep their last layout until the next resolve.
#[test]
fn dropped_in_flow_box_is_not_dereferenced_before_resolve() {
    let (mut doc, ids) = resolve(vec![el("height:20px"), el("height:20px")]);
    doc.mutate().remove_and_drop_node(ids[1]);
    assert_eq!(hit(&doc, 50.0, 10.0), Some(ids[0]));
    assert_ne!(hit(&doc, 50.0, 30.0), Some(ids[1]));
    let body = doc.nodes[ids[0]].parent.unwrap();
    doc.mutate().remove_and_drop_all_children(body);
    assert_ne!(hit(&doc, 50.0, 10.0), Some(ids[0]));
}
