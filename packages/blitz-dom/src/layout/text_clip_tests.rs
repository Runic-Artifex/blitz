//! `background-clip: text`: Blitz clips to the glyphs of the element's own
//! inline formatting context and reports the elements whose text is
//! elsewhere (block or atomic inline descendants, an ordinary inline box).

use blitz_traits::shell::{ColorScheme, Viewport};

use crate::{Attribute, BaseDocument, DocumentConfig, NodeId, qual_name};

const CLIP: &str = "background-image:linear-gradient(red,blue);background-clip:text;-webkit-text-fill-color:transparent";

fn element(
    mutator: &mut crate::DocumentMutator<'_>,
    tag: &str,
    css: &str,
    children: &[NodeId],
) -> NodeId {
    let name = match tag {
        "span" => qual_name!("span"),
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

/// The layout reports of a document whose body holds what `build` returns.
fn reports(build: impl FnOnce(&mut crate::DocumentMutator<'_>) -> NodeId) -> Vec<&'static str> {
    let mut doc = BaseDocument::new(DocumentConfig {
        viewport: Some(Viewport::new(800, 300, 1.0, ColorScheme::Light)),
        font_ctx: Some(crate::build_single_font_ctx(include_bytes!(
            "../../../../examples/wasm_hello/assets/DejaVuSans.woff2"
        ))),
        ..Default::default()
    });
    let root = doc.root_node().id;
    let mut mutator = doc.mutate();
    let tree = build(&mut mutator);
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
    doc.take_unsupported_layout_reports()
}

#[test]
fn text_clip_outside_the_own_inline_formatting_context_is_reported() {
    // Its own text, also in a flex item (shadcn-svelte's Marker.Content).
    assert_eq!(
        reports(|m| {
            let text = m.create_text_node("Generating response");
            element(m, "div", &format!("display:block;{CLIP}"), &[text])
        }),
        Vec::<&str>::new()
    );
    assert_eq!(
        reports(|m| {
            let text = m.create_text_node("Thinking...");
            let item = element(m, "span", CLIP, &[text]);
            element(m, "div", "display:flex", &[item])
        }),
        Vec::<&str>::new()
    );
    // Text in a block child, in an inline-block, in an ordinary inline box.
    for build in [
        (|m: &mut crate::DocumentMutator<'_>| {
            let text = m.create_text_node("Child");
            let child = element(m, "div", "display:block", &[text]);
            element(m, "div", &format!("display:block;{CLIP}"), &[child])
        }) as fn(&mut crate::DocumentMutator<'_>) -> NodeId,
        |m| {
            let text = m.create_text_node("Box");
            let child = element(m, "span", "display:inline-block", &[text]);
            let before = m.create_text_node("Before ");
            element(m, "div", &format!("display:block;{CLIP}"), &[before, child])
        },
        |m| {
            let text = m.create_text_node("Inline");
            let span = element(m, "span", CLIP, &[text]);
            element(m, "div", "display:block", &[span])
        },
    ] {
        assert_eq!(
            reports(build),
            [super::construct::BACKGROUND_CLIP_TEXT_DESCENDANTS]
        );
    }
}
