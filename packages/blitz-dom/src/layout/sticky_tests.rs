//! Sticky positioning against the geometry Chromium 153 gives it (measured in
//! the compiler's harness, `conformance/scrolling.mjs`, DejaVu Sans 16px/20px,
//! an 800x300 page with `margin: 0`).

use blitz_traits::shell::{ColorScheme, Viewport};

use crate::{Attribute, BaseDocument, DocumentConfig, NodeId, ScrollBehavior, qual_name};

const PANE: &str = "display:block;width:300px;height:120px;overflow:auto;scrollbar-width:none;border:2px solid #333";
const STICKY: &str = "display:block;position:sticky;height:24px";

fn element(
    mutator: &mut crate::DocumentMutator<'_>,
    tag: &str,
    css: &str,
    children: &[NodeId],
) -> NodeId {
    let name = match tag {
        "span" => qual_name!("span"),
        "p" => qual_name!("p"),
        "html" => qual_name!("html"),
        "body" => qual_name!("body"),
        _ => qual_name!("div"),
    };
    let id = mutator.create_element(
        name,
        vec![Attribute {
            name: qual_name!("style"),
            value: css.into(),
        }],
    );
    mutator.append_children(id, children);
    id
}

/// A document whose body holds what `build` returns; the ids `build` names.
fn document(
    build: impl FnOnce(&mut crate::DocumentMutator<'_>) -> (NodeId, Vec<NodeId>),
) -> (BaseDocument, Vec<NodeId>) {
    let mut doc = BaseDocument::new(DocumentConfig {
        viewport: Some(Viewport::new(800, 300, 1.0, ColorScheme::Light)),
        font_ctx: Some(crate::build_single_font_ctx(include_bytes!(
            "../../../../examples/wasm_hello/assets/DejaVuSans.woff2"
        ))),
        ..Default::default()
    });
    let root = doc.root_node().id;
    let mut mutator = doc.mutate();
    let (tree, ids) = build(&mut mutator);
    let body = element(
        &mut mutator,
        "body",
        "display:block;margin:0;font:16px/20px sans-serif",
        &[tree],
    );
    let html = element(&mut mutator, "html", "display:block", &[body]);
    mutator.append_children(root, &[html]);
    drop(mutator);
    doc.resolve(0.0);
    (doc, ids)
}

/// The border box top of `id` in the viewport.
fn top(doc: &BaseDocument, id: NodeId) -> f64 {
    doc.get_client_bounding_rect(id).unwrap().y
}

fn scroll(doc: &mut BaseDocument, id: NodeId, y: f64) {
    doc.scroll_to(id, 0.0, y, ScrollBehavior::Instant);
    doc.resolve(0.0);
}

/// The sticky view rectangle is the scrollport without the scroll
/// container's padding (Blink's `ComputeStickyConstrainingRect`): `top: 0`
/// sticks 14px below a padded pane's border, `bottom: 0` 18px above it.
#[test]
fn sticky_view_excludes_scroll_container_padding() {
    let (mut doc, ids) = document(|m| {
        let before = element(m, "div", "display:block;height:40px", &[]);
        let sticky = element(m, "div", &format!("{STICKY};top:0"), &[]);
        let middle = element(m, "div", "display:block;height:150px", &[]);
        let end = element(m, "div", &format!("{STICKY};bottom:0"), &[]);
        let section = element(
            m,
            "div",
            "display:block;height:300px",
            &[before, sticky, middle, end],
        );
        let after = element(m, "div", "display:block;height:100px", &[]);
        let pane = element(
            m,
            "div",
            &format!("{PANE};padding:14px 10px 18px"),
            &[section, after],
        );
        (pane, vec![pane, sticky, end])
    });
    let [pane, sticky, end] = ids[..] else {
        unreachable!()
    };
    // Chromium: the bottom box at 112 (2 + 14 + 120 - 18 - 24 + ...), the
    // top box in flow at 56.
    assert_eq!(top(&doc, end), 112.0);
    assert_eq!(top(&doc, sticky), 56.0);
    scroll(&mut doc, pane, 90.0);
    assert_eq!(top(&doc, sticky), 16.0);
}

/// Percentage insets resolve against the sticky view rectangle's size (the
/// padded pane's 300x120 content box): `top: 10%` is 12px, `left: 5%` 15px.
#[test]
fn sticky_percentages_resolve_against_the_sticky_view() {
    let (mut doc, ids) = document(|m| {
        let before = element(m, "div", "display:block;height:40px", &[]);
        let sticky = element(
            m,
            "div",
            &format!("{STICKY};width:90px;top:10%;left:5%"),
            &[],
        );
        let section = element(
            m,
            "div",
            "display:block;height:400px;width:500px",
            &[before, sticky],
        );
        let pane = element(m, "div", &format!("{PANE};padding:10px"), &[section]);
        (pane, vec![pane, sticky])
    });
    let [pane, sticky] = ids[..] else {
        unreachable!()
    };
    scroll(&mut doc, pane, 200.0);
    let rect = doc.get_client_bounding_rect(sticky).unwrap();
    assert_eq!((rect.x, rect.y), (27.0, 24.0));
}

/// Blink offsets a sticky ordinary inline box by its lines' bounding box;
/// Blitz keeps its fragments in flow, and reports it.
#[test]
fn sticky_inline_box_is_reported() {
    let (mut doc, _) = document(|m| {
        let text = m.create_text_node("inline sticky");
        let span = element(m, "span", "position:sticky;top:5px", &[text]);
        let p = element(m, "p", "display:block", &[span]);
        (p, vec![])
    });
    assert_eq!(
        doc.take_unsupported_layout_reports(),
        [crate::stylo_to_parley::INLINE_STICKY]
    );
    let (mut doc, _) = document(|m| {
        let text = m.create_text_node("inline");
        let span = element(m, "span", "position:sticky", &[text]);
        let p = element(m, "p", "display:block", &[span]);
        (p, vec![])
    });
    assert_eq!(doc.take_unsupported_layout_reports(), Vec::<&str>::new());
}
