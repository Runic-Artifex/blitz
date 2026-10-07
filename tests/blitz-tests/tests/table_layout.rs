//! Table formatting regression cases (no system fonts required).
use blitz_dom::{Document, DocumentConfig};
use blitz_html::HtmlDocument;
use markup5ever::{QualName, local_name, ns};
fn document(markup: &str) -> HtmlDocument {
    let mut doc = HtmlDocument::from_html(
        &format!(
            "<style>body{{margin:0}}table{{border-spacing:2px}}td{{padding:0}}</style>{markup}"
        ),
        DocumentConfig {
            viewport: Some(blitz_traits::shell::Viewport::new(
                640,
                300,
                1.0,
                blitz_traits::shell::ColorScheme::Light,
            )),
            html_parser_provider: Some(std::sync::Arc::new(blitz_html::HtmlProvider)),
            ..Default::default()
        },
    );
    doc.resolve(0.0);
    doc
}
fn rect(doc: &HtmlDocument, selector: &str) -> [f64; 4] {
    let inner = doc.inner();
    let id = inner.query_selector(selector).unwrap().unwrap();
    let r = inner.get_client_bounding_rect(id).unwrap();
    [r.x, r.y, r.width, r.height]
}
const ROW: &str = "<tbody id='group'><tr id='row'><td id='a'><div id='inside' style='width:30px;height:20px'></div></td><td id='b'><div style='width:90px;height:40px'></div></td></tr></tbody>";
#[test]
fn auto_columns_and_internal_boxes() {
    let doc = document(&format!("<table id='table'>{ROW}</table>"));
    assert_eq!(rect(&doc, "#table"), [0.0, 0.0, 126.0, 44.0]);
    assert_eq!(rect(&doc, "#group"), [2.0, 2.0, 122.0, 40.0]);
    assert_eq!(rect(&doc, "#row"), [2.0, 2.0, 122.0, 40.0]);
    assert_eq!(rect(&doc, "#inside"), [2.0, 12.0, 30.0, 20.0]);
    let inner = doc.inner();
    let hit = inner.hit(10.0, 20.0).unwrap();
    let row = inner.query_selector("#row").unwrap().unwrap();
    let mut current = Some(hit.node_id);
    let mut has_row = false;
    while let Some(id) = current {
        has_row |= id == row;
        current = inner.get_node(id).unwrap().parent;
    }
    assert!(
        has_row,
        "row click must target a descendant and bubble through the row"
    );
}
#[test]
fn caption_is_outside_the_grid_box() {
    let doc = document(&format!(
        "<table id='table'><caption id='caption' style='height:10px'></caption>{ROW}</table><div id='after'></div>"
    ));
    assert_eq!(rect(&doc, "#caption"), [0.0, 0.0, 126.0, 10.0]);
    assert_eq!(rect(&doc, "#row"), [2.0, 12.0, 122.0, 40.0]);
    assert_eq!(rect(&doc, "#table"), [0.0, 0.0, 126.0, 54.0]);
    assert_eq!(rect(&doc, "#after")[1], 54.0);
}
#[test]
fn updates_recompute_width_and_do_not_accumulate_alignment_offsets() {
    let mut doc = document(&format!("<table id='table'>{ROW}</table>"));
    for _ in 0..3 {
        let mut inner = doc.inner_mut();
        let id = inner.query_selector("#inside").unwrap().unwrap();
        inner.mutate().set_attribute(
            id,
            QualName::new(None, ns!(), local_name!("style")),
            "width:60px;height:20px",
        );
        drop(inner);
        doc.resolve(0.0);
        assert_eq!(rect(&doc, "#table"), [0.0, 0.0, 156.0, 44.0]);
        assert_eq!(rect(&doc, "#inside"), [2.0, 12.0, 60.0, 20.0]);
    }
}
#[test]
fn rtl_rows_keep_positive_boxes() {
    let doc = document(&format!("<table id='table' dir='rtl'>{ROW}</table>"));
    assert_eq!(rect(&doc, "#row"), [2.0, 2.0, 122.0, 40.0]);
    assert_eq!(rect(&doc, "#a"), [94.0, 2.0, 30.0, 40.0]);
    assert_eq!(rect(&doc, "#b"), [2.0, 2.0, 90.0, 40.0]);
}
#[test]
fn cell_height_is_a_border_box_minimum_and_content_can_grow_it() {
    let mut doc = document(&format!(
        "<style>#a{{height:60px;padding:5px;border:2px solid}}</style><table>{ROW}</table>"
    ));
    assert_eq!(rect(&doc, "#row")[3], 60.0);
    assert_eq!(rect(&doc, "#inside")[1], 22.0);
    let mut inner = doc.inner_mut();
    let id = inner.query_selector("#a").unwrap().unwrap();
    inner.mutate().set_attribute(
        id,
        QualName::new(None, ns!(), local_name!("style")),
        "height:10px",
    );
    drop(inner);
    doc.resolve(0.0);
    assert_eq!(rect(&doc, "#row")[3], 40.0);
    assert_eq!(rect(&doc, "#inside")[1], 12.0);
}

#[test]
fn percentage_cell_height_reserves_space_but_content_can_grow_it() {
    let mut doc = document(
        "<table id='table' style='height:160px'><tr id='row'><td id='a' style='height:50%;padding:4px;border:2px solid'><div id='inside' style='width:30px;height:10px'></div></td></tr><tr id='row2'><td style='padding:4px'><div style='width:30px;height:10px'></div></td></tr></table>",
    );
    assert_eq!(rect(&doc, "#row")[3], 78.0);
    assert_eq!(rect(&doc, "#row2")[3], 76.0);
    let mut inner = doc.inner_mut();
    let id = inner.query_selector("#inside").unwrap().unwrap();
    inner.mutate().set_attribute(
        id,
        QualName::new(None, ns!(), local_name!("style")),
        "width:30px;height:120px",
    );
    drop(inner);
    doc.resolve(0.0);
    assert_eq!(rect(&doc, "#row")[3], 132.0);
    assert_eq!(rect(&doc, "#row2")[3], 22.0);
    let mut inner = doc.inner_mut();
    let id = inner.query_selector("#table").unwrap().unwrap();
    inner.mutate().set_attribute(
        id,
        QualName::new(None, ns!(), local_name!("style")),
        "height:auto",
    );
    drop(inner);
    doc.resolve(0.0);
    assert_eq!(rect(&doc, "#row")[3], 132.0);
    assert_eq!(rect(&doc, "#row2")[3], 18.0);
    assert_eq!(rect(&doc, "#table")[3], 156.0);
}
