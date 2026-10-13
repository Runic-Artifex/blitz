//! `-webkit-line-clamp` boxes against the geometry Chromium 153 gives them in
//! DejaVu Sans at 16px with a 20px line height, on an 800x600 page with
//! `margin: 0` (border boxes relative to the clamp root's).

use blitz_traits::shell::{ColorScheme, Viewport};

use crate::{Attribute, BaseDocument, DocumentConfig, NodeId, qual_name};

/// `display: -webkit-box; -webkit-box-orient: vertical; overflow: hidden`.
const BOX: &str = "overflow:hidden;display:-webkit-box;-webkit-box-orient:vertical;width:200px;";
const PB: &str = "padding:5px 7px;border:1px solid #888;";
const INNER: &str = "padding:4px 0;margin:6px 0 10px;border-bottom:2px solid red;";
const LONG: &str =
    "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen";
const TWELVE: &str = "one two three four five six seven eight nine ten eleven twelve";

enum Tree {
    Div(String, Vec<Tree>),
    Span(&'static str, Vec<Tree>),
    Button(String, Vec<Tree>),
    Br,
    Text(&'static str),
}

fn div(style: &str, children: Vec<Tree>) -> Tree {
    Tree::Div(style.to_string(), children)
}

fn text(text: &'static str) -> Tree {
    Tree::Text(text)
}

/// A clamp root clamping after `lines` lines, with `style` after the clamp.
fn clamp(lines: u32, style: &str, children: Vec<Tree>) -> Tree {
    div(
        &format!("{BOX}-webkit-line-clamp:{lines};{style}"),
        children,
    )
}

fn style(value: &str) -> Vec<Attribute> {
    vec![Attribute {
        name: qual_name!("style"),
        value: value.into(),
    }]
}

fn build(mutator: &mut crate::DocumentMutator<'_>, tree: &Tree) -> NodeId {
    match tree {
        Tree::Div(css, children) => {
            let id =
                mutator.create_element(qual_name!("div"), style(&format!("display:block;{css}")));
            for child in children {
                let child = build(mutator, child);
                mutator.append_children(id, &[child]);
            }
            id
        }
        Tree::Span(css, children) => {
            let id = mutator.create_element(qual_name!("span"), style(css));
            for child in children {
                let child = build(mutator, child);
                mutator.append_children(id, &[child]);
            }
            id
        }
        Tree::Button(css, children) => {
            let id = mutator.create_element(qual_name!("button"), style(css));
            for child in children {
                let child = build(mutator, child);
                mutator.append_children(id, &[child]);
            }
            id
        }
        Tree::Br => mutator.create_element(qual_name!("br"), vec![]),
        Tree::Text(text) => mutator.create_text_node(text),
    }
}

/// The document with `tree` in the body; the id of the tree's root.
fn document(tree: Tree) -> (BaseDocument, NodeId) {
    let mut doc = BaseDocument::new(DocumentConfig {
        viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
        font_ctx: Some(crate::build_single_font_ctx(include_bytes!(
            "../../../../examples/wasm_hello/assets/DejaVuSans.woff2"
        ))),
        ..Default::default()
    });
    let root = doc.root_node().id;
    let mut mutator = doc.mutate();
    let html = mutator.create_element(qual_name!("html"), style("display:block"));
    let body = mutator.create_element(
        qual_name!("body"),
        style("display:block;margin:0;font:16px/20px sans-serif"),
    );
    let id = build(&mut mutator, &tree);
    mutator.append_children(body, &[id]);
    mutator.append_children(html, &[body]);
    mutator.append_children(root, &[html]);
    drop(mutator);
    doc.resolve(0.0);
    (doc, id)
}

fn height(doc: &BaseDocument, id: NodeId) -> f32 {
    doc.get_node(id).unwrap().final_layout().size.height
}

/// The border boxes (top, height) of the element children of `id`.
fn boxes(doc: &BaseDocument, id: NodeId) -> Vec<(f32, f32)> {
    doc.get_node(id)
        .unwrap()
        .children
        .iter()
        .map(|&child| doc.get_node(child).unwrap())
        .filter(|child| child.element_data().is_some())
        .map(|child| {
            let layout = child.final_layout();
            (layout.location.y, layout.size.height)
        })
        .collect()
}

/// The element child `index` of `id`.
fn child(doc: &BaseDocument, id: NodeId, index: usize) -> NodeId {
    doc.get_node(id)
        .unwrap()
        .children
        .iter()
        .copied()
        .filter(|&child| doc.get_node(child).unwrap().element_data().is_some())
        .nth(index)
        .unwrap()
}

/// The clamped line of the inline formatting context `ifc` and the text
/// visible before its ellipsis.
fn ellipsis(doc: &BaseDocument, ifc: NodeId) -> Option<(usize, String)> {
    let clamp = doc.line_clamp_ellipsis(ifc)?;
    let text = doc
        .get_node(ifc)?
        .element_data()?
        .inline_layout_data
        .as_ref()?;
    let line = text.layout.get(clamp.line)?;
    let mut visible = String::new();
    for item in line.items() {
        let parley::PositionedLayoutItem::GlyphRun(run) = item else {
            continue;
        };
        let mut x = run.offset();
        for cluster in run.run().visual_clusters() {
            x += cluster.advance();
            if clamp.cutoff.is_none_or(|cutoff| x <= cutoff + 0.01) {
                visible.push_str(&text.text[cluster.text_range()]);
            }
        }
    }
    let visible = match clamp.cutoff {
        Some(_) => visible,
        None => visible.trim_end().to_string(),
    };
    Some((clamp.line, visible))
}

#[test]
fn clamps_text_after_lines() {
    // Two lines of four, and the padding and border around them.
    let (doc, a) = document(clamp(2, PB, vec![text(LONG)]));
    assert_eq!(height(&doc, a), 52.0);
    assert_eq!(
        ellipsis(&doc, a),
        Some((1, "six seven eight nine te".into()))
    );
    // The later lines remain in the scrollable overflow (`scrollHeight` 90).
    let layout = doc.get_node(a).unwrap().unrounded_layout();
    assert_eq!(layout.scrollable_overflow_rect.bottom, 90.0);

    // One line.
    let (doc, h) = document(clamp(1, "", vec![text("short")]));
    assert_eq!(height(&doc, h), 20.0);
    assert_eq!(ellipsis(&doc, h), None);
}

#[test]
fn does_not_clamp_without_content_after_the_clamp_line() {
    // Two lines, and exactly three, with a clamp of three.
    let (doc, b) = document(clamp(
        3,
        "",
        vec![text("one two three four five six seven eight nine")],
    ));
    assert_eq!(height(&doc, b), 40.0);
    assert_eq!(ellipsis(&doc, b), None);
    let (doc, b2) = document(clamp(3, "", vec![text(TWELVE)]));
    assert_eq!(height(&doc, b2), 60.0);
    assert_eq!(ellipsis(&doc, b2), None);
}

#[test]
fn places_the_ellipsis_after_a_line_that_fits() {
    let (doc, m) = document(clamp(
        2,
        "",
        vec![
            text("one two three"),
            Tree::Br,
            text("four"),
            Tree::Br,
            text("five six"),
        ],
    ));
    assert_eq!(height(&doc, m), 40.0);
    assert_eq!(ellipsis(&doc, m), Some((1, "four".into())));
}

#[test]
fn clamps_through_nested_blocks() {
    let (doc, d) = document(clamp(
        2,
        PB,
        vec![
            div(INNER, vec![text("first block")]),
            div(
                INNER,
                vec![text("second block that wraps onto lines and lines")],
            ),
            div(INNER, vec![text("third")]),
        ],
    ));
    assert_eq!(height(&doc, d), 88.0);
    assert_eq!(boxes(&doc, d), [(12.0, 30.0), (52.0, 30.0), (92.0, 30.0)]);
    let second = child(&doc, d, 1);
    assert_eq!(ellipsis(&doc, second).map(|(line, _)| line), Some(0));

    // A block after the clamped one.
    let (doc, e) = document(clamp(
        2,
        "",
        vec![
            div(INNER, vec![text("first block that wraps once more")]),
            div("", vec![text("tail")]),
        ],
    ));
    assert_eq!(height(&doc, e), 56.0);
    assert_eq!(boxes(&doc, e), [(6.0, 50.0), (66.0, 20.0)]);

    // Nothing after it: not clamped, its end margin included.
    let (doc, f) = document(clamp(
        2,
        "",
        vec![div(INNER, vec![text("first block that wraps once more")])],
    ));
    assert_eq!(height(&doc, f), 66.0);
    assert_eq!(ellipsis(&doc, child(&doc, f, 0)), None);

    // An empty block after it is content after the clamp point.
    let (doc, g) = document(clamp(
        2,
        "",
        vec![
            div(INNER, vec![text("first block that wraps once more")]),
            div("", vec![]),
        ],
    ));
    assert_eq!(height(&doc, g), 56.0);
}

#[test]
fn shortens_blocks_after_the_clamp_point() {
    // Each block past the clamp point ends after its first line or child.
    let (doc, r) = document(clamp(
        1,
        "overflow:visible",
        vec![
            div("", vec![text("first")]),
            div(
                INNER,
                vec![text(
                    "a b c d e f g h i j k l m n o p q r s t u v w x y z a b c d e f",
                )],
            ),
            div("", vec![text("tail")]),
        ],
    ));
    assert_eq!(height(&doc, r), 20.0);
    assert_eq!(boxes(&doc, r), [(0.0, 20.0), (26.0, 30.0), (66.0, 20.0)]);

    let (doc, s) = document(clamp(
        2,
        "overflow:visible",
        vec![
            div("", vec![text("one")]),
            div(
                "",
                vec![
                    div("", vec![text("two")]),
                    div(
                        "",
                        vec![text(
                            "three three three three three three three three three",
                        )],
                    ),
                ],
            ),
            div("", vec![text("tail")]),
        ],
    ));
    assert_eq!(height(&doc, s), 40.0);
    assert_eq!(boxes(&doc, s), [(0.0, 20.0), (20.0, 20.0), (40.0, 20.0)]);
    let s1 = child(&doc, s, 1);
    assert_eq!(boxes(&doc, s1), [(0.0, 20.0), (20.0, 20.0)]);
    // `scrollHeight` 100 and 80.
    assert_eq!(
        doc.get_node(s)
            .unwrap()
            .unrounded_layout()
            .scrollable_overflow_rect
            .bottom,
        100.0
    );
    assert_eq!(
        doc.get_node(s1)
            .unwrap()
            .unrounded_layout()
            .scrollable_overflow_rect
            .bottom,
        80.0
    );
}

#[test]
fn counts_lines_only_in_the_formatting_context() {
    // The lines of a formatting context root inside are not counted.
    let (doc, n) = document(clamp(
        2,
        "",
        vec![
            div(
                "display:flow-root",
                vec![text(
                    "flow root text one two three four five six seven eight",
                )],
            ),
            text("text after the root one two three four"),
        ],
    ));
    assert_eq!(height(&doc, n), 100.0);

    // Lines before a block, then the block.
    let (doc, o) = document(clamp(
        2,
        "",
        vec![
            text("text before the block one two three"),
            div("", vec![text("block one two three four five six")]),
        ],
    ));
    assert_eq!(height(&doc, o), 40.0);

    // A taller atomic inline box on the first line.
    let (doc, l) = document(clamp(
        2,
        "",
        vec![
            text("one two "),
            Tree::Span(
                "display:inline-block;width:30px;height:30px;background:red",
                vec![],
            ),
            text(" three four five six seven eight nine ten"),
        ],
    ));
    assert_eq!(height(&doc, l), 55.0);
}

#[test]
fn keeps_fixed_and_limited_block_sizes() {
    let (doc, i) = document(clamp(2, "height:70px", vec![text(TWELVE)]));
    assert_eq!(height(&doc, i), 70.0);
    assert_eq!(ellipsis(&doc, i).map(|(line, _)| line), Some(1));
    let (doc, j) = document(clamp(2, "min-height:70px", vec![text(TWELVE)]));
    assert_eq!(height(&doc, j), 70.0);
    let (doc, u) = document(clamp(2, "max-height:30px", vec![text(TWELVE)]));
    assert_eq!(height(&doc, u), 30.0);
    let (doc, v) = document(clamp(
        1,
        "min-height:50px",
        vec![div("", vec![text("one")]), div("", vec![text("two")])],
    ));
    assert_eq!(height(&doc, v), 50.0);
    assert_eq!(boxes(&doc, v), [(0.0, 20.0), (20.0, 20.0)]);
}

#[test]
fn clamps_in_other_contexts() {
    // A flex item, and the item after it.
    let (doc, flex) = document(div(
        "display:flex;flex-direction:column;width:260px",
        vec![
            clamp(2, &format!("{PB}margin-bottom:8px"), vec![text(TWELVE)]),
            div("", vec![text("after")]),
        ],
    ));
    assert_eq!(boxes(&doc, flex), [(0.0, 52.0), (60.0, 20.0)]);

    // Overflow visible.
    let (doc, p) = document(clamp(
        2,
        &format!("{PB}overflow:visible"),
        vec![text(
            "one two three four five six seven eight nine ten eleven twelve thirteen",
        )],
    ));
    assert_eq!(height(&doc, p), 52.0);

    // `-webkit-inline-box`: an inline-block.
    let (doc, q) = document(div(
        "",
        vec![clamp(
            2,
            "width:auto;display:-webkit-inline-box",
            vec![text(
                "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty twentyone twentytwo twentythree",
            )],
        )],
    ));
    let q = child(&doc, q, 0);
    let layout = doc.get_node(q).unwrap().final_layout();
    assert_eq!((layout.size.width, layout.size.height), (800.0, 40.0));
}

#[test]
fn does_not_clamp_other_displays() {
    // `display: flex` after `-webkit-box`: no clamp.
    let (doc, flex) = document(clamp(2, "display:flex", vec![text(LONG)]));
    assert_eq!(height(&doc, flex), 80.0);
    // A horizontal box: no clamp.
    let (doc, horizontal) = document(div(
        "display:flow-root;width:200px;-webkit-line-clamp:2",
        vec![text(LONG)],
    ));
    assert_eq!(height(&doc, horizontal), 80.0);
}

#[test]
fn follows_text_changes() {
    let (mut doc, a) = document(clamp(2, PB, vec![text(LONG)]));
    assert_eq!(height(&doc, a), 52.0);
    let text_node = doc.get_node(a).unwrap().children[0];
    doc.mutate().set_node_text(text_node, "short");
    doc.resolve(0.0);
    assert_eq!(height(&doc, a), 32.0);
    assert_eq!(ellipsis(&doc, a), None);
    doc.mutate().set_node_text(text_node, LONG);
    doc.resolve(0.0);
    assert_eq!(height(&doc, a), 52.0);
    assert_eq!(
        ellipsis(&doc, a),
        Some((1, "six seven eight nine te".into()))
    );

    // Nested: the clamp moves when the first block grows.
    let (mut doc, d) = document(clamp(
        2,
        "",
        vec![
            div("", vec![text("one")]),
            div("", vec![text("two")]),
            div("", vec![text("three")]),
        ],
    ));
    assert_eq!(height(&doc, d), 40.0);
    let first = child(&doc, d, 0);
    assert_eq!(
        ellipsis(&doc, child(&doc, d, 1)).map(|(line, _)| line),
        Some(0)
    );
    let first_text = doc.get_node(first).unwrap().children[0];
    doc.mutate()
        .set_node_text(first_text, "one two three four five six seven eight nine");
    doc.resolve(0.0);
    assert_eq!(height(&doc, d), 40.0);
    assert_eq!(boxes(&doc, d), [(0.0, 40.0), (40.0, 20.0), (60.0, 20.0)]);
    assert_eq!(ellipsis(&doc, first).map(|(line, _)| line), Some(1));
    assert_eq!(ellipsis(&doc, child(&doc, d, 1)), None);
}

#[test]
fn reports_legacy_boxes() {
    let (mut doc, _) = document(clamp(2, PB, vec![text(LONG)]));
    assert_eq!(doc.take_unsupported_layout_reports(), Vec::<&str>::new());
    // Chromium lays these out as legacy flexible boxes.
    for style in [
        "display:-webkit-box;-webkit-line-clamp:2",
        "display:-webkit-box;-webkit-box-orient:vertical",
        "display:-webkit-box",
    ] {
        let (mut doc, _) = document(div(style, vec![text(LONG)]));
        assert_eq!(
            doc.take_unsupported_layout_reports(),
            [super::LEGACY_WEBKIT_BOX],
            "{style}"
        );
    }
    let (mut doc, _) = document(div(
        "",
        vec![Tree::Button(
            format!("{BOX}-webkit-line-clamp:2"),
            vec![text(LONG)],
        )],
    ));
    assert_eq!(
        doc.take_unsupported_layout_reports(),
        [super::LINE_CLAMP_BUTTON]
    );
}
