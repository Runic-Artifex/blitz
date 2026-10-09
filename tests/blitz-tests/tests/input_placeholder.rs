//! `<input placeholder>` is laid out and shown while the field is empty.
//!
//! The attribute was previously ignored entirely, so an empty field rendered
//! as a blank box with no hint of what belonged in it.

use blitz_dom::{Document, DocumentConfig};
use blitz_html::{HtmlDocument, HtmlProvider};
use blitz_traits::shell::{ColorScheme, Viewport};
use markup5ever::{QualName, local_name, ns};
use std::sync::Arc;

fn document(html: &str) -> HtmlDocument {
    let mut doc = HtmlDocument::from_html(
        html,
        DocumentConfig {
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            html_parser_provider: Some(Arc::new(HtmlProvider) as _),
            ..Default::default()
        },
    );
    doc.resolve(0.0);
    doc
}

/// `(is the placeholder laid out, is it currently shown, the value)`
fn state(doc: &HtmlDocument, id: &str) -> (bool, bool, String) {
    let node = doc.get_element_by_id(id).unwrap();
    let input = doc.tree()[node]
        .element_data()
        .and_then(|el| el.text_input_data())
        .expect("an input has text input data");

    (
        input.placeholder.is_some(),
        input.shows_placeholder(),
        input.editor.text().to_string(),
    )
}

fn placeholder_color(doc: &HtmlDocument, id: &str) -> [f32; 4] {
    let node = doc.get_element_by_id(id).unwrap();
    doc.tree()[node]
        .element_data()
        .and_then(|el| el.text_input_data())
        .and_then(|input| input.placeholder.as_ref())
        .expect("the field has a placeholder")
        .color
        .components
}

#[test]
fn an_empty_input_shows_its_placeholder() {
    let doc = document(r#"<html><body><input id="a" placeholder="Search"></body></html>"#);
    let (laid_out, shown, value) = state(&doc, "a");

    assert!(laid_out, "the placeholder is laid out");
    assert!(shown, "an empty field shows the placeholder");
    assert_eq!(value, "", "the placeholder is never part of the value");
}

#[test]
fn a_value_hides_the_placeholder() {
    let doc =
        document(r#"<html><body><input id="a" value="Ada" placeholder="Search"></body></html>"#);
    let (laid_out, shown, value) = state(&doc, "a");

    assert!(laid_out);
    assert!(!shown, "a field with a value does not show the placeholder");
    assert_eq!(value, "Ada");
}

#[test]
fn an_input_without_a_placeholder_has_none() {
    let doc = document(r#"<html><body><input id="a"></body></html>"#);
    let (laid_out, shown, value) = state(&doc, "a");

    assert!(!laid_out);
    assert!(!shown);
    assert_eq!(
        value, "",
        "an empty input holds an empty string, not a space"
    );
}

#[test]
fn an_empty_placeholder_attribute_is_ignored() {
    let doc = document(r#"<html><body><input id="a" placeholder=""></body></html>"#);
    let (laid_out, shown, _) = state(&doc, "a");

    assert!(!laid_out);
    assert!(!shown);
}

#[test]
fn a_textarea_shows_its_placeholder() {
    let doc =
        document(r#"<html><body><textarea id="a" placeholder="Notes"></textarea></body></html>"#);
    let (laid_out, shown, _) = state(&doc, "a");

    assert!(laid_out);
    assert!(shown);
}

#[test]
fn placeholder_text_is_dimmer_than_the_value_by_default() {
    let doc = document(r##"<html><body><input id="a" placeholder="Search"></body></html>"##);

    let [.., alpha] = placeholder_color(&doc, "a");
    assert!(
        alpha < 1.0,
        "the UA sheet dims placeholder text, got alpha {alpha}"
    );
}

#[test]
fn a_stylesheet_can_restyle_the_placeholder() {
    let doc = document(
        r##"<html><head><style>
            #a::placeholder { color: rgb(255, 0, 0); }
        </style></head><body><input id="a" placeholder="Search"></body></html>"##,
    );

    let [red, green, blue, alpha] = placeholder_color(&doc, "a");
    assert_eq!(
        [red, green, blue, alpha],
        [1.0, 0.0, 0.0, 1.0],
        "::placeholder overrides the UA colour"
    );
}

fn dimensions(doc: &HtmlDocument, id: &str) -> (f32, f32) {
    let node = doc.get_element_by_id(id).unwrap();
    let size = doc.get_node(node).unwrap().final_layout().size;
    (size.width, size.height)
}

fn type_attribute(doc: &HtmlDocument, id: &str) -> Option<String> {
    let node = doc.get_element_by_id(id).unwrap();
    doc.tree()[node]
        .element_data()
        .unwrap()
        .attr(local_name!("type"))
        .map(str::to_owned)
}

#[test]
fn missing_invalid_empty_and_mixed_case_types_have_text_geometry() {
    let doc = document(
        r#"<html><body>
        <input id="text" type="text" size="7" placeholder="Search">
        <input id="missing" size="7" placeholder="Search">
        <input id="unknown" type="not-a-type" size="7" placeholder="Search">
        <input id="empty" type="" size="7" placeholder="Search">
        <input id="mixed" type="TeXt" size="7" placeholder="Search">
        <input id="obsolete" type="datetime" size="7" placeholder="Search">
        <input id="spaced" type=" hidden " size="7" placeholder="Search">
        <input id="wide" type="text" size="21" placeholder="Search">
        </body></html>"#,
    );
    let expected = dimensions(&doc, "text");
    assert!(expected.0 > 0.0 && expected.1 > 0.0);
    assert!(
        dimensions(&doc, "wide").0 > expected.0,
        "size controls intrinsic width"
    );
    for (id, authored) in [
        ("missing", None),
        ("unknown", Some("not-a-type")),
        ("empty", Some("")),
        ("mixed", Some("TeXt")),
        ("obsolete", Some("datetime")),
        ("spaced", Some(" hidden ")),
    ] {
        assert_eq!(state(&doc, id), (true, true, String::new()), "{id}");
        assert_eq!(
            dimensions(&doc, id),
            expected,
            "intrinsic text geometry for {id}"
        );
        assert_eq!(
            type_attribute(&doc, id).as_deref(),
            authored,
            "raw type for {id}"
        );
    }
}

#[test]
fn mixed_case_editor_types_keep_their_existing_layout_domains() {
    for keyword in [
        "text", "password", "email", "tel", "url", "search", "number",
    ] {
        let mixed = keyword.to_ascii_uppercase();
        let doc = document(&format!(
            "<input id='lower' type='{keyword}' value='12' placeholder='Hint'>\
             <input id='mixed' type='{mixed}' value='12' placeholder='Hint'>"
        ));
        assert_eq!(
            state(&doc, "mixed"),
            (true, false, "12".to_owned()),
            "{keyword}"
        );
        assert_eq!(
            dimensions(&doc, "mixed"),
            dimensions(&doc, "lower"),
            "{keyword}"
        );
        assert_eq!(type_attribute(&doc, "mixed"), Some(mixed));
    }
}

#[test]
fn known_nontext_types_do_not_receive_text_field_padding_or_editors() {
    for keyword in [
        "date",
        "month",
        "week",
        "time",
        "datetime-local",
        "number",
        "range",
        "color",
        "checkbox",
        "radio",
        "file",
        "submit",
        "image",
        "reset",
        "button",
    ] {
        let authored = keyword.to_ascii_uppercase();
        let doc = document(&format!("<input id='a' type='{authored}'>"));
        let node = doc.get_element_by_id("a").unwrap();
        let has_editor = doc.tree()[node]
            .element_data()
            .unwrap()
            .text_input_data()
            .is_some();
        // Number keeps its pre-existing editor, but does not use text-field leaf sizing.
        assert_eq!(
            has_editor,
            keyword == "number",
            "editor domain for {keyword}"
        );
        let expected_padding = if matches!(keyword, "submit" | "reset" | "button") {
            "6px"
        } else {
            "0px"
        };
        assert_eq!(
            doc.resolved_style_value(node, "padding-left"),
            expected_padding,
            "UA domain for {keyword}"
        );
        assert_eq!(type_attribute(&doc, "a"), Some(authored));
    }
}

#[test]
fn mixed_case_hidden_and_checkable_types_keep_widget_geometry() {
    let doc = document(
        r#"<input id="hidden" type="HiDdEn" value="secret" placeholder="Hint">
        <input id="checkbox" type="checkbox" checked><input id="mixed-checkbox" type="ChEcKbOx" checked>
        <input id="radio" type="radio" checked><input id="mixed-radio" type="RaDiO" checked>"#,
    );
    assert_eq!(dimensions(&doc, "hidden"), (0.0, 0.0));
    let hidden = doc.get_element_by_id("hidden").unwrap();
    assert!(
        doc.tree()[hidden]
            .element_data()
            .unwrap()
            .text_input_data()
            .is_none()
    );
    for (lower, mixed) in [("checkbox", "mixed-checkbox"), ("radio", "mixed-radio")] {
        assert_eq!(dimensions(&doc, mixed), (13.0, 13.0));
        assert_eq!(dimensions(&doc, mixed), dimensions(&doc, lower));
        let node = doc.get_element_by_id(mixed).unwrap();
        assert_eq!(
            doc.tree()[node]
                .element_data()
                .unwrap()
                .checkbox_input_checked(),
            Some(true)
        );
    }
    assert_eq!(type_attribute(&doc, "hidden").as_deref(), Some("HiDdEn"));
}

#[test]
fn textarea_ignores_an_input_type_attribute() {
    let doc = document(
        r#"<textarea id="plain" placeholder="Notes">Ada</textarea>
        <textarea id="typed" type="HiDdEn" placeholder="Notes">Ada</textarea>"#,
    );
    assert_eq!(state(&doc, "typed"), (true, false, "Ada".to_owned()));
    assert_eq!(dimensions(&doc, "typed"), dimensions(&doc, "plain"));
    let node = doc.get_element_by_id("typed").unwrap();
    assert!(
        doc.tree()[node]
            .element_data()
            .unwrap()
            .text_input_data()
            .unwrap()
            .is_multiline
    );
    assert_eq!(type_attribute(&doc, "typed").as_deref(), Some("HiDdEn"));
}

#[test]
fn changing_text_to_invalid_and_back_preserves_live_value_and_intrinsic_geometry() {
    let mut doc =
        document(r#"<input id="a" type="text" size="11" value="Authored" placeholder="Search">"#);
    let node = doc.get_element_by_id("a").unwrap();
    let expected = dimensions(&doc, "a");
    assert!(expected.0 > 0.0 && expected.1 > 0.0);
    doc.with_text_input(node, |mut driver| {
        driver.select_all();
        driver.insert_or_replace_selection("Live value");
    });
    for authored in ["not-a-type", "text"] {
        doc.inner_mut().mutate().set_attribute(
            node,
            QualName::new(None, ns!(), local_name!("type")),
            authored,
        );
        doc.resolve(0.0);
        assert_eq!(state(&doc, "a"), (true, false, "Live value".to_owned()));
        assert_eq!(dimensions(&doc, "a"), expected);
        assert_eq!(type_attribute(&doc, "a").as_deref(), Some(authored));
        assert_eq!(
            doc.tree()[node]
                .element_data()
                .unwrap()
                .attr(local_name!("value")),
            Some("Authored")
        );
    }
    doc.with_text_input(node, |mut driver| {
        driver.select_all();
        driver.delete_selection();
    });
    doc.inner_mut().mutate().set_attribute(
        node,
        QualName::new(None, ns!(), local_name!("type")),
        "still-invalid",
    );
    doc.resolve(0.0);
    assert_eq!(state(&doc, "a"), (true, true, String::new()));
    assert_eq!(dimensions(&doc, "a"), expected);
}
