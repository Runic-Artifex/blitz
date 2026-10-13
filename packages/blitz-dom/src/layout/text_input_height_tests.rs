//! The block size of single-line text inputs, and of text areas in
//! auto-height column flex containers, against Chromium 153 in DejaVu Sans
//! (an 800x600 page, `margin: 0`).
//!
//! A text input's content height is its inner editor's line: the editor's
//! line height is `normal` (the font's) when the input's computed line height
//! is at most its font size, or when its `height` is a percentage or a length
//! above that line height (Blink's `TextControlInnerEditorElement`). It sizes
//! the input when its `height` is `auto` or does not apply, and whenever the
//! input is sized by its content: a content flex basis (`flex: 1 1 0%` in an
//! auto-height column) or its automatic minimum size.

use blitz_traits::shell::{ColorScheme, Viewport};

use crate::{Attribute, BaseDocument, DocumentConfig, NodeId, qual_name};

fn attrs(pairs: &[(&str, &str)]) -> Vec<Attribute> {
    pairs
        .iter()
        .map(|(name, value)| Attribute {
            name: match *name {
                "placeholder" => qual_name!("placeholder"),
                _ => qual_name!("style"),
            },
            value: (*value).into(),
        })
        .collect()
}

/// `<div style="height:200px"><div style="{group}">{control}</div></div>` in
/// `<body style="margin:0">`, a text area followed by a 50px block; returns
/// the group's and the control's border-box heights.
fn heights(group: &str, tag: &str, control: &[(&str, &str)]) -> (f32, f32) {
    let mut doc = BaseDocument::new(DocumentConfig {
        viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
        font_ctx: Some(crate::build_single_font_ctx(include_bytes!(
            "../../../../examples/wasm_hello/assets/DejaVuSans.woff2"
        ))),
        ..Default::default()
    });
    let root = doc.root_node().id;
    let mut m = doc.mutate();
    let html = m.create_element(qual_name!("html", html), vec![]);
    let body = m.create_element(qual_name!("body", html), attrs(&[("style", "margin:0")]));
    let outer = m.create_element(qual_name!("div", html), attrs(&[("style", "height:200px")]));
    let g = m.create_element(qual_name!("div", html), attrs(&[("style", group)]));
    let name = match tag {
        "textarea" => qual_name!("textarea", html),
        _ => qual_name!("input", html),
    };
    let c: NodeId = m.create_element(name, attrs(control));
    let mut children = vec![c];
    if tag == "textarea" {
        children
            .push(m.create_element(qual_name!("div", html), attrs(&[("style", "height:50px")])));
    }
    m.append_children(g, &children);
    m.append_children(outer, &[g]);
    m.append_children(body, &[outer]);
    m.append_children(html, &[body]);
    m.append_children(root, &[html]);
    drop(m);
    doc.resolve(0.0);
    (
        doc.nodes[g].unrounded_layout().size.height,
        doc.nodes[c].unrounded_layout().size.height,
    )
}

/// (font size, line-height, height, context, input height, group height),
/// measured in Chromium 153.0.8010.12. The input has `padding: 12px 8px 4px;
/// border: 0; box-sizing: border-box; width: 100%`. Contexts: `flex0` is
/// `flex: 1 1 0%` and `flexauto` `flex: 1 1 auto` in an auto-height 400px
/// column flex container, `block` a 400px block, `flexmin` `flex: 0 1 auto`
/// in a 20px-high column flex container (the input shrinks to its automatic
/// minimum size).
#[rustfmt::skip]
const INPUTS: &[(f32, &str, &str, &str, f32, f32)] = &[
    (14.0, "normal", "auto", "flex0", 32.0, 32.0),
    (14.0, "normal", "auto", "flexauto", 32.0, 32.0),
    (14.0, "normal", "auto", "block", 32.0, 32.0),
    (14.0, "normal", "auto", "flexmin", 32.0, 20.0),
    (14.0, "normal", "36px", "flex0", 32.0, 32.0),
    (14.0, "normal", "36px", "flexauto", 36.0, 36.0),
    (14.0, "normal", "36px", "block", 36.0, 36.0),
    (14.0, "normal", "36px", "flexmin", 32.0, 20.0),
    (14.0, "normal", "50%", "flex0", 32.0, 32.0),
    (14.0, "normal", "50%", "flexauto", 32.0, 32.0),
    (14.0, "normal", "50%", "block", 32.0, 32.0),
    (14.0, "normal", "50%", "flexmin", 16.0, 20.0),
    (14.0, "20px", "auto", "flex0", 36.0, 36.0),
    (14.0, "20px", "auto", "flexauto", 36.0, 36.0),
    (14.0, "20px", "auto", "block", 36.0, 36.0),
    (14.0, "20px", "auto", "flexmin", 36.0, 20.0),
    (14.0, "20px", "36px", "flex0", 32.0, 32.0),
    (14.0, "20px", "36px", "flexauto", 36.0, 36.0),
    (14.0, "20px", "36px", "block", 36.0, 36.0),
    (14.0, "20px", "36px", "flexmin", 32.0, 20.0),
    (14.0, "20px", "50%", "flex0", 32.0, 32.0),
    (14.0, "20px", "50%", "flexauto", 32.0, 32.0),
    (14.0, "20px", "50%", "block", 32.0, 32.0),
    (14.0, "20px", "50%", "flexmin", 16.0, 20.0),
    (14.0, "10px", "auto", "flex0", 32.0, 32.0),
    (14.0, "10px", "auto", "flexauto", 32.0, 32.0),
    (14.0, "10px", "auto", "block", 32.0, 32.0),
    (14.0, "10px", "auto", "flexmin", 32.0, 20.0),
    (14.0, "10px", "36px", "flex0", 32.0, 32.0),
    (14.0, "10px", "36px", "flexauto", 36.0, 36.0),
    (14.0, "10px", "36px", "block", 36.0, 36.0),
    (14.0, "10px", "36px", "flexmin", 32.0, 20.0),
    (14.0, "10px", "50%", "flex0", 32.0, 32.0),
    (14.0, "10px", "50%", "flexauto", 32.0, 32.0),
    (14.0, "10px", "50%", "block", 32.0, 32.0),
    (14.0, "10px", "50%", "flexmin", 16.0, 20.0),
    (18.0, "normal", "auto", "flex0", 37.0, 37.0),
    (18.0, "normal", "auto", "flexauto", 37.0, 37.0),
    (18.0, "normal", "auto", "block", 37.0, 37.0),
    (18.0, "normal", "auto", "flexmin", 37.0, 20.0),
    (18.0, "normal", "36px", "flex0", 37.0, 37.0),
    (18.0, "normal", "36px", "flexauto", 36.0, 36.0),
    (18.0, "normal", "36px", "block", 36.0, 36.0),
    (18.0, "normal", "36px", "flexmin", 36.0, 20.0),
    (18.0, "normal", "50%", "flex0", 37.0, 37.0),
    (18.0, "normal", "50%", "flexauto", 37.0, 37.0),
    (18.0, "normal", "50%", "block", 37.0, 37.0),
    (18.0, "normal", "50%", "flexmin", 16.0, 20.0),
    (18.0, "20px", "auto", "flex0", 36.0, 36.0),
    (18.0, "20px", "auto", "flexauto", 36.0, 36.0),
    (18.0, "20px", "auto", "block", 36.0, 36.0),
    (18.0, "20px", "auto", "flexmin", 36.0, 20.0),
    (18.0, "20px", "36px", "flex0", 37.0, 37.0),
    (18.0, "20px", "36px", "flexauto", 36.0, 36.0),
    (18.0, "20px", "36px", "block", 36.0, 36.0),
    (18.0, "20px", "36px", "flexmin", 36.0, 20.0),
    (18.0, "20px", "50%", "flex0", 37.0, 37.0),
    (18.0, "20px", "50%", "flexauto", 37.0, 37.0),
    (18.0, "20px", "50%", "block", 37.0, 37.0),
    (18.0, "20px", "50%", "flexmin", 16.0, 20.0),
    (18.0, "10px", "auto", "flex0", 37.0, 37.0),
    (18.0, "10px", "auto", "flexauto", 37.0, 37.0),
    (18.0, "10px", "auto", "block", 37.0, 37.0),
    (18.0, "10px", "auto", "flexmin", 37.0, 20.0),
    (18.0, "10px", "36px", "flex0", 37.0, 37.0),
    (18.0, "10px", "36px", "flexauto", 36.0, 36.0),
    (18.0, "10px", "36px", "block", 36.0, 36.0),
    (18.0, "10px", "36px", "flexmin", 36.0, 20.0),
    (18.0, "10px", "50%", "flex0", 37.0, 37.0),
    (18.0, "10px", "50%", "flexauto", 37.0, 37.0),
    (18.0, "10px", "50%", "block", 37.0, 37.0),
    (18.0, "10px", "50%", "flexmin", 16.0, 20.0),
];

#[test]
fn text_input_content_height_is_its_editor_line() {
    let mut failures = Vec::new();
    for &(font_size, line_height, height, context, input, group) in INPUTS {
        let flex = match context {
            "flex0" => "flex:1 1 0%;",
            "flexauto" => "flex:1 1 auto;",
            "flexmin" => "flex:0 1 auto;",
            _ => "",
        };
        let container = match context {
            "block" => "width:400px",
            "flexmin" => "display:flex;flex-direction:column;width:400px;height:20px",
            _ => "display:flex;flex-direction:column;width:400px",
        };
        let style = format!(
            "font:{font_size}px sans-serif;line-height:{line_height};margin:0;padding:12px 8px 4px;\
             border:0;box-sizing:border-box;height:{height};width:100%;{flex}"
        );
        let got = heights(container, "input", &[("style", &style)]);
        if got != (group, input) {
            failures.push(format!(
                "font {font_size} line-height {line_height} height {height} {context}: \
                 (group, input) {got:?}, Chromium ({group}, {input})"
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// A growing text area (`min-height: 64px`; two rows, or one line of
/// placeholder with `field-sizing: content`) above a 50px block in a 400px
/// column flex container with a 1px border: Chromium 153 makes the container
/// 132px and the text area 80px with either `field-sizing` and any of these
/// flex values. Blitz's Taffy fork counted the scroll container's flex base
/// size without its `min-height` (108px and 88px with `flex: 1 1 0%`).
#[test]
fn growing_text_area_in_auto_height_column() {
    for sizing in ["fixed", "content"] {
        for flex in ["flex:1 1 0%;", "flex:1 1 auto;", ""] {
            let style = format!(
                "font:14px sans-serif;line-height:20px;margin:0;padding:8px;border:0;resize:none;\
                 width:100%;min-height:64px;{flex}field-sizing:{sizing}"
            );
            let got = heights(
                "display:flex;flex-direction:column;width:400px;border:1px solid",
                "textarea",
                &[("style", &style), ("placeholder", "Write a comment...")],
            );
            assert_eq!(got, (132.0, 80.0), "field-sizing: {sizing}, {flex:?}");
        }
    }
}
