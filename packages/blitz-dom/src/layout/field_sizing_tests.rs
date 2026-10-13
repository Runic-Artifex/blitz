//! Text controls with `field-sizing: content`, against the border boxes
//! Chromium 153 gives them in DejaVu Sans at 16px (an 800x600 page, the
//! control in the body or in the wrapper given, `margin: 0`; text inputs
//! with `line-height: 20px`).

use blitz_traits::shell::{ColorScheme, Viewport};

use crate::{Attribute, BaseDocument, DocumentConfig, NodeId, qual_name};

/// Without padding and border, so that the box is the content's.
const BARE: &str = "padding:0;border:0;resize:none;";
const CONTENT: &str = "field-sizing:content;";

fn attrs(pairs: &[(&str, &str)]) -> Vec<Attribute> {
    pairs
        .iter()
        .map(|(name, value)| Attribute {
            name: match *name {
                "placeholder" => qual_name!("placeholder"),
                "value" => qual_name!("value"),
                "rows" => qual_name!("rows"),
                "cols" => qual_name!("cols"),
                "size" => qual_name!("size"),
                "type" => qual_name!("type"),
                _ => qual_name!("style"),
            },
            value: (*value).into(),
        })
        .collect()
}

/// A control in `<body style="margin:0">`, inside `<div style="{wrapper}">`
/// if given: `<input>` or `<textarea>` (with `text`), with `style` after
/// `font: 16px sans-serif; margin: 0` and the other attributes given.
fn document(
    wrapper: Option<&str>,
    tag: &str,
    style: &str,
    attributes: &[(&str, &str)],
    text: Option<&str>,
) -> (BaseDocument, NodeId) {
    let mut doc = BaseDocument::new(DocumentConfig {
        viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
        font_ctx: Some(crate::build_single_font_ctx(include_bytes!(
            "../../../../examples/wasm_hello/assets/DejaVuSans.woff2"
        ))),
        ..Default::default()
    });
    // Text inputs get an explicit line height: with `normal`, Blitz gives a
    // text input a fractional line box (docs/BLITZ-GAPS.md, "Text input line
    // height"), with or without `field-sizing`.
    let line_height = if tag == "input" {
        "line-height:20px;"
    } else {
        ""
    };
    let style = format!("font:16px sans-serif;{line_height}margin:0;{style}");
    let mut all = vec![("style", style.as_str())];
    all.extend_from_slice(attributes);
    let root = doc.root_node().id;
    let mut mutator = doc.mutate();
    let html = mutator.create_element(qual_name!("html", html), vec![]);
    let body = mutator.create_element(qual_name!("body", html), attrs(&[("style", "margin:0")]));
    let name = match tag {
        "textarea" => qual_name!("textarea", html),
        _ => qual_name!("input", html),
    };
    let control = mutator.create_element(name, attrs(&all));
    if let Some(text) = text {
        let text = mutator.create_text_node(text);
        mutator.append_children(control, &[text]);
    }
    match wrapper {
        Some(style) => {
            let div = mutator.create_element(qual_name!("div", html), attrs(&[("style", style)]));
            mutator.append_children(div, &[control]);
            mutator.append_children(body, &[div]);
        }
        None => mutator.append_children(body, &[control]),
    }
    mutator.append_children(html, &[body]);
    mutator.append_children(root, &[html]);
    drop(mutator);
    doc.resolve(0.0);
    (doc, control)
}

fn size(doc: &BaseDocument, id: NodeId) -> (f32, f32) {
    let size = doc.nodes[id].unrounded_layout().size;
    (size.width, size.height)
}

fn input(style: &str, attributes: &[(&str, &str)]) -> (f32, f32) {
    let (doc, id) = document(None, "input", style, attributes, None);
    size(&doc, id)
}

fn textarea(style: &str, attributes: &[(&str, &str)], text: &str) -> (f32, f32) {
    let (doc, id) = document(None, "textarea", style, attributes, Some(text));
    size(&doc, id)
}

fn bare(style: &str) -> String {
    format!("{BARE}{CONTENT}{style}")
}

#[test]
fn input_is_as_wide_as_its_text() {
    // An empty control is the caret's width.
    assert_eq!(input(&bare(""), &[]), (1.0, 20.0));
    assert_eq!(input(&bare(""), &[("value", "a")]), (9.8125, 20.0));
    assert_eq!(
        input(&bare(""), &[("value", "Hello world")]),
        (89.703125, 20.0)
    );
    // The UA's padding and border are added.
    assert_eq!(
        input(CONTENT, &[("value", "Hello world")]),
        (97.703125, 26.0)
    );
    assert_eq!(input(CONTENT, &[]), (9.0, 26.0));
    // Spaces are preserved, trailing ones included (the inner editor is
    // `white-space: pre`).
    assert_eq!(input(&bare(""), &[("value", "ab   ")]), (35.21875, 20.0));
    assert_eq!(input(&bare(""), &[("value", "a   b")]), (35.21875, 20.0));
    // `size` is ignored.
    assert_eq!(
        input(&bare(""), &[("size", "40"), ("value", "abc")]),
        (28.765625, 20.0)
    );
    for kind in ["email", "url", "tel"] {
        assert_eq!(
            input(&bare(""), &[("type", kind), ("value", "a@b.c")]),
            (49.84375, 20.0),
            "{kind}"
        );
    }
}

#[test]
fn input_placeholder_counts_while_shown() {
    assert_eq!(
        input(&bare(""), &[("placeholder", "Placeholder text")]),
        (130.15625, 20.0)
    );
    assert_eq!(
        input(
            &bare(""),
            &[("placeholder", "Placeholder text"), ("value", "Hi")]
        ),
        (16.484375, 20.0)
    );
    assert_eq!(
        input(
            &bare(""),
            &[
                ("placeholder", "Placeholder"),
                ("value", "A value longer than its placeholder")
            ]
        ),
        (280.453125, 20.0)
    );
}

#[test]
fn input_min_max_and_width() {
    let abc = &[("value", "abc")];
    assert_eq!(input(&bare("min-width:100px"), abc), (100.0, 20.0));
    assert_eq!(
        input(&bare("max-width:60px"), &[("value", "Hello world wide")]),
        (60.0, 20.0)
    );
    assert_eq!(
        input(&bare("width:150px"), &[("value", "Hello")]),
        (150.0, 20.0)
    );
    assert_eq!(input(&bare("line-height:30px"), abc), (28.765625, 30.0));
    let flex = |style: &str| {
        let (doc, id) = document(
            Some("display:flex;width:300px"),
            "input",
            &bare(style),
            abc,
            None,
        );
        size(&doc, id)
    };
    assert_eq!(flex(""), (28.765625, 20.0));
    assert_eq!(flex("flex:1 1 0%;min-width:0"), (300.0, 20.0));
}

#[test]
fn textarea_is_as_large_as_its_text() {
    // One line and the caret's width when empty.
    assert_eq!(textarea(&bare(""), &[], ""), (1.0, 19.0));
    assert_eq!(textarea(CONTENT, &[], ""), (7.0, 25.0));
    assert_eq!(textarea(&bare(""), &[], "a"), (9.8125, 19.0));
    assert_eq!(textarea(&bare(""), &[], "Hello world"), (89.703125, 19.0));
    assert_eq!(
        textarea(&bare(""), &[], "line one\nline two is longer"),
        (137.015625, 38.0)
    );
    assert_eq!(textarea(&bare(""), &[], "abc\n"), (28.765625, 38.0));
    // Spaces are preserved (`white-space: pre-wrap`), trailing ones count.
    assert_eq!(textarea(&bare(""), &[], "ab   "), (35.21875, 19.0));
    assert_eq!(textarea(&bare(""), &[], "a   b"), (35.21875, 19.0));
    // `rows` and `cols` are ignored.
    assert_eq!(
        textarea(&bare(""), &[("rows", "6"), ("cols", "40")], "abc"),
        (28.765625, 19.0)
    );
    assert_eq!(
        textarea(&bare("line-height:20px;font-size:14px"), &[], "a\nb"),
        (8.890625, 40.0)
    );
}

#[test]
fn textarea_wraps_at_its_width() {
    const TEXT: &str = "The quick brown fox jumps over the lazy dog and keeps running";
    assert_eq!(textarea(&bare("width:200px"), &[], TEXT), (200.0, 57.0));
    // The placeholder wraps too.
    assert_eq!(
        textarea(&bare("width:200px"), &[("placeholder", TEXT)], ""),
        (200.0, 57.0)
    );
    assert_eq!(
        textarea(&bare(""), &[("placeholder", "Type your message here.")], ""),
        (201.03125, 19.0)
    );
    // `overflow-wrap: break-word` (the UA's) breaks a word wider than the box.
    assert_eq!(
        textarea(&bare("width:100px"), &[], "Internationalization"),
        (100.0, 38.0)
    );
    assert_eq!(
        textarea(
            &bare("max-width:120px"),
            &[],
            "The quick brown fox jumps over the lazy dog"
        ),
        (120.0, 76.0)
    );
}

#[test]
fn textarea_min_max_and_intrinsic_contributions() {
    assert_eq!(
        textarea(&bare("min-height:64px"), &[], "abc"),
        (28.765625, 64.0)
    );
    assert_eq!(
        textarea(
            &bare("max-height:40px;overflow:hidden"),
            &[],
            "one\ntwo\nthree\nfour"
        ),
        (42.328125, 40.0)
    );
    assert_eq!(
        textarea(&bare("min-width:120px"), &[], "abc"),
        (120.0, 19.0)
    );
    // The shadcn-svelte Textarea's box: border-box, a minimum height.
    assert_eq!(
        textarea(
            &format!(
                "{CONTENT}box-sizing:border-box;padding:8px 12px;border:1px solid;min-height:64px;width:300px;resize:none"
            ),
            &[("placeholder", "Type your message here.")],
            ""
        ),
        (300.0, 64.0)
    );
    let wrapped = |wrapper: &str, style: &str, text: &str| {
        let (doc, id) = document(Some(wrapper), "textarea", &bare(style), &[], Some(text));
        size(&doc, id)
    };
    assert_eq!(
        wrapped("display:flex;width:300px", "", "abc def"),
        (59.484375, 19.0)
    );
    // A percentage width still contributes the text's min-content width.
    assert_eq!(
        wrapped("width:min-content", "width:100%", "abc defgh"),
        (45.9375, 38.0)
    );
}

/// Typing grows and shrinks the box, as in Chromium after each step.
#[test]
fn typing_resizes() {
    let (mut doc, id) = document(
        None,
        "textarea",
        &bare("width:200px;min-height:30px"),
        &[("placeholder", "Say something")],
        Some(""),
    );
    let mut step =
        |doc: &mut BaseDocument,
         edit: &dyn Fn(&mut parley::PlainEditorDriver<'_, crate::node::TextBrush>)| {
            doc.with_text_input(id, |mut driver| edit(&mut driver));
            doc.resolve(0.0);
            size(doc, id)
        };
    assert_eq!(size(&doc, id), (200.0, 30.0));
    assert_eq!(
        step(&mut doc, &|d| d.insert_or_replace_selection("Hello")),
        (200.0, 30.0)
    );
    assert_eq!(
        step(&mut doc, &|d| d.insert_or_replace_selection(
            " there, this text wraps onto more lines"
        )),
        (200.0, 38.0)
    );
    assert_eq!(
        step(&mut doc, &|d| d.insert_or_replace_selection("\n")),
        (200.0, 57.0)
    );
    assert_eq!(step(&mut doc, &|d| d.backdelete()), (200.0, 38.0));
    assert_eq!(
        step(&mut doc, &|d| {
            d.select_all();
            d.delete_selection();
        }),
        (200.0, 30.0)
    );

    let (mut doc, id) = document(None, "textarea", &bare(""), &[], Some(""));
    let mut step =
        |doc: &mut BaseDocument,
         edit: &dyn Fn(&mut parley::PlainEditorDriver<'_, crate::node::TextBrush>)| {
            doc.with_text_input(id, |mut driver| edit(&mut driver));
            doc.resolve(0.0);
            size(doc, id)
        };
    assert_eq!(size(&doc, id), (1.0, 19.0));
    assert_eq!(
        step(&mut doc, &|d| d.insert_or_replace_selection("abc")),
        (28.765625, 19.0)
    );
    assert_eq!(
        step(&mut doc, &|d| d.insert_or_replace_selection("\n")),
        (28.765625, 38.0)
    );
    assert_eq!(
        step(&mut doc, &|d| d.insert_or_replace_selection("de")),
        (28.765625, 38.0)
    );
    assert_eq!(step(&mut doc, &|d| d.backdelete()), (28.765625, 38.0));
    assert_eq!(step(&mut doc, &|d| d.backdelete()), (28.765625, 38.0));
    assert_eq!(step(&mut doc, &|d| d.backdelete()), (28.765625, 19.0));

    let (mut doc, id) = document(
        None,
        "input",
        &bare("min-width:20px;max-width:200px"),
        &[("placeholder", "Name")],
        None,
    );
    let mut step =
        |doc: &mut BaseDocument,
         edit: &dyn Fn(&mut parley::PlainEditorDriver<'_, crate::node::TextBrush>)| {
            doc.with_text_input(id, |mut driver| edit(&mut driver));
            doc.resolve(0.0);
            size(doc, id)
        };
    assert_eq!(size(&doc, id), (47.203125, 20.0));
    assert_eq!(
        step(&mut doc, &|d| d.insert_or_replace_selection("Jo")),
        (20.0, 20.0)
    );
    assert_eq!(
        step(&mut doc, &|d| d
            .insert_or_replace_selection("hn Jacob Jingleheimer Schmidt")),
        (200.0, 20.0)
    );
    assert_eq!(
        step(&mut doc, &|d| {
            d.select_all();
            d.delete_selection();
        }),
        (47.203125, 20.0)
    );
}

/// The fixed sizes for reference: `field-sizing: fixed` (the initial value)
/// keeps `size` and `rows`.
#[test]
fn fixed_sizing_is_unchanged() {
    assert_eq!(input(BARE, &[("value", "a")]), (200.0, 20.0));
    assert_eq!(
        input(&format!("{BARE}field-sizing:fixed"), &[("value", "a")]),
        (200.0, 20.0)
    );
}
