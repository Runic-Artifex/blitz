//! The `load` and `error` outcomes of `<img>` elements' image requests
//! ([`BaseDocument::take_image_load_events`]).

use crate::{
    Attribute, BaseDocument, DocumentConfig, ImageLoadEvent, ImageLoadEventKind, NodeId, qual_name,
};
use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};
use blitz_traits::shell::{ColorScheme, Viewport};
use std::sync::{Arc, Mutex};

/// A `NetProvider` which records requests so a test completes them when
/// and how it chooses.
#[derive(Default)]
struct ManualNetProvider {
    requests: Mutex<Vec<(String, Box<dyn NetHandler>)>>,
}

impl NetProvider for ManualNetProvider {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        self.requests
            .lock()
            .unwrap()
            .push((request.url.to_string(), handler));
    }
}

impl ManualNetProvider {
    fn urls(&self) -> Vec<String> {
        let requests = self.requests.lock().unwrap();
        requests.iter().map(|(url, _)| url.clone()).collect()
    }

    fn take(&self, url: &str) -> Box<dyn NetHandler> {
        let mut requests = self.requests.lock().unwrap();
        let index = requests
            .iter()
            .position(|(requested, _)| requested == url)
            .unwrap_or_else(|| panic!("no request for {url}"));
        requests.remove(index).1
    }
}

const A: &str = "https://example.test/a.png";
const B: &str = "https://example.test/b.png";

fn png(width: u32, height: u32) -> Bytes {
    let mut bytes = Vec::new();
    image::RgbaImage::new(width, height)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    Bytes::from(bytes)
}

fn load(node_id: NodeId) -> ImageLoadEvent {
    ImageLoadEvent {
        node_id,
        kind: ImageLoadEventKind::Load,
    }
}

fn error(node_id: NodeId) -> ImageLoadEvent {
    ImageLoadEvent {
        node_id,
        kind: ImageLoadEventKind::Error,
    }
}

struct Fixture {
    doc: BaseDocument,
    net: Arc<ManualNetProvider>,
    body: NodeId,
}

impl Fixture {
    fn new() -> Self {
        let net = Arc::new(ManualNetProvider::default());
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            base_url: Some("https://example.test/".into()),
            net_provider: Some(net.clone() as _),
            ..Default::default()
        });
        doc.add_user_agent_stylesheet("html,body{display:block;margin:0}");
        let root = doc.root_node().id;
        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html", html), vec![]);
        let body = mutator.create_element(qual_name!("body", html), vec![]);
        mutator.append_children(html, &[body]);
        mutator.append_children(root, &[html]);
        drop(mutator);
        Self { doc, net, body }
    }

    /// A detached `<img>`, with `src` if given.
    fn create_img(&mut self, src: Option<&str>) -> NodeId {
        let attrs = src
            .map(|src| Attribute {
                name: qual_name!("src"),
                value: src.into(),
            })
            .into_iter()
            .collect();
        self.doc
            .mutate()
            .create_element(qual_name!("img", html), attrs)
    }

    /// An `<img>` with `src` appended to the body.
    fn img(&mut self, src: &str) -> NodeId {
        let img = self.create_img(Some(src));
        self.append(img);
        img
    }

    fn append(&mut self, node_id: NodeId) {
        let body = self.body;
        self.doc.mutate().append_children(body, &[node_id]);
    }

    fn set_src(&mut self, img: NodeId, src: &str) {
        self.doc.mutate().set_attribute(img, qual_name!("src"), src);
    }

    /// Process the responses delivered so far, then take the outcomes.
    fn events(&mut self) -> Vec<ImageLoadEvent> {
        self.doc.handle_messages();
        self.doc.take_image_load_events()
    }

    fn has_image(&self, img: NodeId) -> bool {
        self.doc.nodes[img]
            .element_data()
            .unwrap()
            .image_data()
            .is_some()
    }
}

#[test]
fn decoded_image_fires_load_after_its_response_is_processed() {
    let mut f = Fixture::new();
    let img = f.img(A);
    assert_eq!(f.net.urls(), [A]);
    assert_eq!(f.doc.take_image_load_events(), []);

    f.net.take(A).bytes(A.into(), png(30, 20));
    // The outcome is determined when the document processes the response.
    assert_eq!(f.doc.take_image_load_events(), []);
    assert_eq!(f.events(), [load(img)]);
    assert_eq!(f.events(), []);

    // The image is laid out at its intrinsic size.
    f.doc.resolve(0.0);
    let size = f.doc.nodes[img].final_layout().size;
    assert_eq!((size.width, size.height), (30.0, 20.0));
}

#[test]
fn resolve_processes_responses() {
    let mut f = Fixture::new();
    let img = f.img(A);
    f.net.take(A).bytes(A.into(), png(3, 2));
    f.doc.resolve(0.0);
    assert_eq!(f.doc.take_image_load_events(), [load(img)]);
}

#[test]
fn failed_fetch_fires_error() {
    let mut f = Fixture::new();
    let img = f.img(A);
    f.net.take(A).error(A.into(), "HTTP 404".into());
    assert_eq!(f.events(), [error(img)]);
    assert!(!f.has_image(img));
}

#[test]
fn failed_decode_fires_error() {
    let mut f = Fixture::new();
    let img = f.img(A);
    f.net
        .take(A)
        .bytes(A.into(), Bytes::from_static(b"not an image"));
    assert_eq!(f.events(), [error(img)]);
    assert!(!f.has_image(img));
}

#[test]
fn every_element_waiting_for_a_response_gets_its_outcome_in_order() {
    let mut f = Fixture::new();
    let first = f.img(A);
    let second = f.img(A);
    // One request serves both.
    assert_eq!(f.net.urls(), [A]);
    f.net.take(A).bytes(A.into(), png(3, 2));
    assert_eq!(f.events(), [load(first), load(second)]);
}

#[test]
fn loaded_image_fires_load_when_src_is_set() {
    let mut f = Fixture::new();
    let first = f.img(A);
    f.net.take(A).bytes(A.into(), png(3, 2));
    assert_eq!(f.events(), [load(first)]);

    // Already loaded: the outcome is known without a request.
    let second = f.img(A);
    assert_eq!(f.net.urls(), Vec::<String>::new());
    assert_eq!(f.doc.take_image_load_events(), [load(second)]);
    assert!(f.has_image(second));

    // Setting `src` again, even to the same value, fires `load` again.
    f.set_src(first, A);
    assert_eq!(f.doc.take_image_load_events(), [load(first)]);
}

#[test]
fn redirected_response_is_applied_to_the_waiting_element() {
    let mut f = Fixture::new();
    let img = f.img(A);
    f.net
        .take(A)
        .bytes("https://cdn.example.test/a.png".into(), png(3, 2));
    assert_eq!(f.events(), [load(img)]);
    assert!(f.has_image(img));
}

#[test]
fn changing_src_abandons_the_earlier_request() {
    let mut f = Fixture::new();
    let img = f.img(A);
    f.set_src(img, B);
    assert_eq!(f.net.urls(), [A, B]);

    // The earlier request's response is neither applied nor reported.
    f.net.take(A).bytes(A.into(), png(3, 2));
    assert_eq!(f.events(), []);
    assert!(!f.has_image(img));
    f.net.take(B).bytes(B.into(), png(5, 4));
    assert_eq!(f.events(), [load(img)]);

    let img = f.img(B);
    assert_eq!(f.doc.take_image_load_events(), [load(img)]);
    f.set_src(img, "https://example.test/c.png");
    f.net
        .take("https://example.test/c.png")
        .error("https://example.test/c.png".into(), "HTTP 404".into());
    assert_eq!(f.events(), [error(img)]);
    // A failed request leaves no stale image.
    assert!(!f.has_image(img));
}

#[test]
fn changing_src_drops_the_outcome_not_yet_taken() {
    let mut f = Fixture::new();
    let first = f.img(A);
    f.net.take(A).bytes(A.into(), png(3, 2));
    assert_eq!(f.events(), [load(first)]);

    let img = f.img(A);
    // The `load` for A was not taken yet when `src` changed.
    f.set_src(img, B);
    assert_eq!(f.doc.take_image_load_events(), []);
    f.net.take(B).error(B.into(), "HTTP 404".into());
    assert_eq!(f.events(), [error(img)]);
}

#[test]
fn removing_src_abandons_the_request_without_an_event() {
    let mut f = Fixture::new();
    let img = f.img(A);
    f.doc.mutate().clear_attribute(img, qual_name!("src"));
    f.net.take(A).bytes(A.into(), png(3, 2));
    assert_eq!(f.events(), []);
    assert!(!f.has_image(img));
}

#[test]
fn empty_or_invalid_src_fires_error() {
    let mut f = Fixture::new();
    let empty = f.img("");
    let invalid = f.img("https://[invalid");
    assert_eq!(f.net.urls(), Vec::<String>::new());
    assert_eq!(
        f.doc.take_image_load_events(),
        [error(empty), error(invalid)]
    );
}

#[test]
fn img_without_src_fires_nothing() {
    let mut f = Fixture::new();
    let img = f.create_img(None);
    f.append(img);
    assert_eq!(f.net.urls(), Vec::<String>::new());
    assert_eq!(f.events(), []);
}

#[test]
fn reinserting_an_element_requests_its_image_only_if_src_changed() {
    let mut f = Fixture::new();
    let img = f.img(A);
    f.net.take(A).bytes(A.into(), png(3, 2));
    assert_eq!(f.events(), [load(img)]);

    // Moving the element does not request its image again.
    f.doc.mutate().remove_node(img);
    f.append(img);
    assert_eq!(f.events(), []);
    assert!(f.has_image(img));

    // A `src` set while it was outside the document is requested on insertion,
    // and the earlier request is abandoned.
    f.doc.mutate().remove_node(img);
    f.set_src(img, B);
    assert_eq!(f.net.urls(), Vec::<String>::new());
    f.append(img);
    assert_eq!(f.net.urls(), [B]);
    f.net.take(B).bytes(B.into(), png(5, 4));
    assert_eq!(f.events(), [load(img)]);
}

#[test]
fn removed_element_keeps_its_pending_request() {
    let mut f = Fixture::new();
    let img = f.img(A);
    f.doc.mutate().remove_node(img);
    f.net.take(A).bytes(A.into(), png(3, 2));
    assert_eq!(f.events(), [load(img)]);
}

#[test]
fn dropped_element_reports_nothing() {
    let mut f = Fixture::new();
    let loaded = f.img(A);
    f.net.take(A).bytes(A.into(), png(3, 2));
    f.doc.handle_messages();
    let pending = f.img(B);
    f.doc.mutate().remove_and_drop_node(loaded);
    f.doc.mutate().remove_and_drop_node(pending);
    f.net.take(B).bytes(B.into(), png(3, 2));
    assert_eq!(f.events(), []);
}
