//! CSS blend modes (Compositing and Blending 1) as peniko blend modes.
//!
//! Both renderers (vello_cpu and vello) blend separated sRGB colour values
//! with the W3C formulas, as Chromium does; `plus-lighter` is the clamped
//! premultiplied sum, peniko's `Compose::PlusLighter`.

use peniko::{BlendMode, Compose, Mix};
use style::computed_values::background_blend_mode::single_value::T as BackgroundBlend;
use style::computed_values::mix_blend_mode::T as MixBlend;

/// The blend mode of `mix-blend-mode`, or `None` for `normal`.
pub(crate) fn mix_blend_mode(value: MixBlend) -> Option<BlendMode> {
    let mix = match value {
        MixBlend::Normal => return None,
        MixBlend::PlusLighter => return Some(BlendMode::new(Mix::Normal, Compose::PlusLighter)),
        MixBlend::Multiply => Mix::Multiply,
        MixBlend::Screen => Mix::Screen,
        MixBlend::Overlay => Mix::Overlay,
        MixBlend::Darken => Mix::Darken,
        MixBlend::Lighten => Mix::Lighten,
        MixBlend::ColorDodge => Mix::ColorDodge,
        MixBlend::ColorBurn => Mix::ColorBurn,
        MixBlend::HardLight => Mix::HardLight,
        MixBlend::SoftLight => Mix::SoftLight,
        MixBlend::Difference => Mix::Difference,
        MixBlend::Exclusion => Mix::Exclusion,
        MixBlend::Hue => Mix::Hue,
        MixBlend::Saturation => Mix::Saturation,
        MixBlend::Color => Mix::Color,
        MixBlend::Luminosity => Mix::Luminosity,
    };
    Some(BlendMode::new(mix, Compose::SrcOver))
}

/// The blend mode of one `background-blend-mode` value, or `None` for
/// `normal`.
pub(crate) fn background_blend_mode(value: BackgroundBlend) -> Option<BlendMode> {
    let mix = match value {
        BackgroundBlend::Normal => return None,
        BackgroundBlend::Multiply => Mix::Multiply,
        BackgroundBlend::Screen => Mix::Screen,
        BackgroundBlend::Overlay => Mix::Overlay,
        BackgroundBlend::Darken => Mix::Darken,
        BackgroundBlend::Lighten => Mix::Lighten,
        BackgroundBlend::ColorDodge => Mix::ColorDodge,
        BackgroundBlend::ColorBurn => Mix::ColorBurn,
        BackgroundBlend::HardLight => Mix::HardLight,
        BackgroundBlend::SoftLight => Mix::SoftLight,
        BackgroundBlend::Difference => Mix::Difference,
        BackgroundBlend::Exclusion => Mix::Exclusion,
        BackgroundBlend::Hue => Mix::Hue,
        BackgroundBlend::Saturation => Mix::Saturation,
        BackgroundBlend::Color => Mix::Color,
        BackgroundBlend::Luminosity => Mix::Luminosity,
    };
    Some(BlendMode::new(mix, Compose::SrcOver))
}

#[cfg(test)]
mod tests {
    //! Blending drawn by the CPU renderer against Chromium 153's pixels
    //! (backdrop `rgb(40, 120, 200)`, source `rgb(220, 90, 60)`).

    use anyrender::render_to_buffer;
    use anyrender_vello_cpu::VelloCpuImageRenderer;
    use blitz_dom::{Attribute, BaseDocument, DocumentConfig, LocalName, QualName, ns};
    use blitz_traits::shell::{ColorScheme, Viewport};

    const W: u32 = 640;
    const H: u32 = 120;
    const B: &str = "rgb(40, 120, 200)";
    const S: &str = "rgb(220, 90, 60)";
    /// `multiply` of S over B, and S itself (blended with an isolated,
    /// transparent backdrop).
    const MULTIPLIED: [u8; 3] = [35, 43, 47];
    const UNBLENDED: [u8; 3] = [220, 90, 60];

    struct El {
        tag: &'static str,
        style: String,
        children: Vec<El>,
    }

    fn el(tag: &'static str, style: impl Into<String>, children: Vec<El>) -> El {
        El {
            tag,
            style: style.into(),
            children,
        }
    }

    /// An absolutely positioned box at `left`, `top` of 40x40 px.
    fn abs(left: u32, top: u32, style: &str, children: Vec<El>) -> El {
        el(
            "div",
            format!("position:absolute;left:{left}px;top:{top}px;width:40px;height:40px;{style}"),
            children,
        )
    }

    fn fill(style: &str) -> El {
        el("div", format!("width:40px;height:40px;{style}"), vec![])
    }

    fn render(css: &str, html_style: &str, body: Vec<El>) -> Vec<u8> {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(W, H, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        if !css.is_empty() {
            doc.add_user_agent_stylesheet(css);
        }
        let root = doc.root_node().id;
        let mut mutator = doc.mutate();
        fn build(mutator: &mut blitz_dom::DocumentMutator<'_>, el: &El) -> blitz_dom::NodeId {
            let name = QualName::new(None, ns!(html), LocalName::from(el.tag));
            let style = Attribute {
                name: QualName::new(None, ns!(), LocalName::from("style")),
                value: el.style.clone(),
            };
            let id = mutator.create_element(name, vec![style]);
            for child in &el.children {
                let child = build(mutator, child);
                mutator.append_children(id, &[child]);
            }
            id
        }
        let html = build(
            &mut mutator,
            &el("html", html_style, vec![el("body", "margin:0", body)]),
        );
        mutator.append_children(root, &[html]);
        drop(mutator);
        doc.resolve(0.0);
        render_to_buffer::<VelloCpuImageRenderer, _>(|scene| paint_on_canvas(scene, &mut doc), W, H)
    }

    /// Paints the document over a white canvas, as browsers (and the Runic
    /// runtime) do.
    fn paint_on_canvas(scene: &mut impl anyrender::PaintScene, doc: &mut BaseDocument) {
        scene.fill(
            peniko::Fill::NonZero,
            kurbo::Affine::IDENTITY,
            peniko::Color::WHITE,
            None,
            &kurbo::Rect::new(0.0, 0.0, f64::from(W), f64::from(H)),
        );
        crate::paint_scene(scene, doc, 1.0, W, H, 0, 0);
    }

    fn pixel(buffer: &[u8], x: u32, y: u32) -> [u8; 3] {
        let i = ((y * W + x) * 4) as usize;
        [buffer[i], buffer[i + 1], buffer[i + 2]]
    }

    #[track_caller]
    fn assert_near(actual: [u8; 3], expected: [u8; 3], what: &str) {
        let close = actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 1);
        assert!(close, "{what}: {actual:?}, Chromium {expected:?}");
    }

    #[test]
    fn modes_match_chromium() {
        let modes: [(&str, [u8; 3]); 17] = [
            ("normal", [220, 90, 60]),
            ("multiply", [35, 43, 47]),
            ("screen", [225, 168, 213]),
            ("overlay", [69, 85, 171]),
            ("darken", [40, 90, 60]),
            ("lighten", [220, 120, 200]),
            ("color-dodge", [255, 185, 255]),
            ("color-burn", [6, 0, 21]),
            ("hard-light", [196, 85, 94]),
            ("soft-light", [84, 101, 177]),
            ("difference", [180, 30, 140]),
            ("exclusion", [190, 124, 166]),
            ("hue", [199, 69, 39]),
            ("saturation", [40, 120, 200]),
            ("color", [199, 69, 39]),
            ("luminosity", [61, 141, 221]),
            ("plus-lighter", [255, 210, 255]),
        ];
        let mut body = vec![];
        for (i, (mode, _)) in modes.iter().enumerate() {
            let left = 10 + i as u32 * 30;
            body.push(abs(
                left,
                10,
                &format!("width:20px;height:20px;background:{B}"),
                vec![],
            ));
            body.push(abs(
                left,
                10,
                &format!("width:20px;height:20px;background:{S};mix-blend-mode:{mode}"),
                vec![],
            ));
        }
        let buffer = render("", "", body);
        for (i, (mode, expected)) in modes.iter().enumerate() {
            let x = 20 + i as u32 * 30;
            assert_near(pixel(&buffer, x, 20), *expected, mode);
        }
    }

    /// Clips that do not create a stacking context do not isolate; stacking
    /// contexts do (measured in Chromium 153 for each ancestor).
    #[test]
    fn isolated_groups_match_chromium() {
        let ancestors: [(&str, [u8; 3]); 10] = [
            ("", MULTIPLIED),
            ("overflow:hidden", MULTIPLIED),
            ("overflow:hidden;border-radius:6px", MULTIPLIED),
            ("overflow:auto", MULTIPLIED),
            ("clip:rect(0px,40px,40px,0px)", MULTIPLIED),
            ("isolation:isolate", UNBLENDED),
            ("opacity:0.99", [217, 90, 61]),
            ("transform:translateX(0px)", UNBLENDED),
            ("z-index:0", UNBLENDED),
            ("filter:blur(0px)", UNBLENDED),
        ];
        let mut body = vec![];
        for (i, (style, _)) in ancestors.iter().enumerate() {
            let left = 10 + i as u32 * 50;
            body.push(abs(left, 10, &format!("background:{B}"), vec![]));
            let blended = fill(&format!("background:{S};mix-blend-mode:multiply"));
            body.push(abs(left, 10, style, vec![blended]));
        }
        // Opacity without a blend inside, for the renderer's own rounding of
        // the 0.99 opacity (Chromium: 217, 90, 61; vello_cpu: 218, 91, 63).
        body.push(abs(10, 60, &format!("background:{B}"), vec![]));
        let unblended = fill(&format!("background:{S}"));
        body.push(abs(10, 60, "opacity:0.99", vec![unblended]));
        let buffer = render("", "", body);
        for (i, (style, expected)) in ancestors.iter().enumerate() {
            let x = 30 + i as u32 * 50;
            let expected = if style.starts_with("opacity") {
                pixel(&buffer, 30, 80)
            } else {
                *expected
            };
            assert_near(pixel(&buffer, x, 30), expected, style);
        }
    }

    /// A blended box positioned against an overflow clip that is no stacking
    /// context (painted from the root's stacking context with the clip
    /// applied again) blends with the clip's content and with what is
    /// behind it.
    #[test]
    fn hoisted_child_blends_through_clip() {
        let content = vec![
            el(
                "div",
                "width:20px;height:40px;background:rgb(0, 255, 0)",
                vec![],
            ),
            abs(
                0,
                0,
                &format!("width:60px;background:{S};mix-blend-mode:multiply"),
                vec![],
            ),
        ];
        let body = vec![
            abs(10, 10, &format!("background:{B}"), vec![]),
            abs(10, 10, "overflow:hidden", content),
        ];
        let buffer = render("", "", body);
        assert_near(
            pixel(&buffer, 15, 30),
            [0, 90, 0],
            "over the clip's content",
        );
        assert_near(pixel(&buffer, 40, 30), MULTIPLIED, "over the backdrop");
        // Clipped by the ancestor: the 60px box ends at its padding box.
        assert_near(pixel(&buffer, 60, 30), [255, 255, 255], "outside the clip");
    }

    #[test]
    fn group_composition_matches_chromium() {
        let body = vec![
            // Opacity with a blend on the same element.
            abs(10, 10, &format!("background:{B}"), vec![]),
            abs(
                10,
                10,
                &format!("background:{S};mix-blend-mode:multiply;opacity:0.5"),
                vec![],
            ),
            // A child overflowing the blended element is blended with it.
            abs(60, 10, &format!("width:80px;background:{B}"), vec![]),
            abs(
                60,
                10,
                "mix-blend-mode:multiply",
                vec![fill(&format!("width:80px;background:{S}"))],
            ),
            // A blend inside a blended element blends with its group only.
            abs(150, 10, &format!("background:{B}"), vec![]),
            abs(
                150,
                10,
                "mix-blend-mode:screen",
                vec![fill(&format!("background:{S};mix-blend-mode:multiply"))],
            ),
            // A blended element over the canvas, blending with its background.
            abs(
                200,
                10,
                &format!("background:{S};mix-blend-mode:difference"),
                vec![],
            ),
        ];
        let buffer = render("", "", body);
        assert_near(pixel(&buffer, 30, 30), [38, 82, 124], "opacity");
        assert_near(pixel(&buffer, 130, 30), MULTIPLIED, "overflowing child");
        assert_near(pixel(&buffer, 170, 30), [225, 168, 213], "nested blend");
        assert_near(pixel(&buffer, 220, 30), [35, 165, 195], "over the canvas");
    }

    #[test]
    fn pseudo_element_blends() {
        let css = format!(
            ".ring::after {{ content: ''; position: absolute; inset: 0; background: {S}; mix-blend-mode: darken }}"
        );
        let ring = abs(10, 10, &format!("background:{B}"), vec![]);
        let buffer = render_with_class(&css, ring, "ring");
        assert_near(pixel(&buffer, 30, 30), [40, 90, 60], "::after darken");
    }

    /// A body with one element of the given class.
    fn render_with_class(css: &str, first: El, class: &str) -> Vec<u8> {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(W, H, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        doc.add_user_agent_stylesheet(css);
        let root = doc.root_node().id;
        let mut mutator = doc.mutate();
        let attr = |name: &str, value: &str| Attribute {
            name: QualName::new(None, ns!(), LocalName::from(name)),
            value: value.to_string(),
        };
        let qual = |tag: &str| QualName::new(None, ns!(html), LocalName::from(tag));
        let html = mutator.create_element(qual("html"), vec![]);
        let body_id = mutator.create_element(qual("body"), vec![attr("style", "margin:0")]);
        let div = mutator.create_element(
            qual(first.tag),
            vec![attr("style", &first.style), attr("class", class)],
        );
        mutator.append_children(body_id, &[div]);
        mutator.append_children(html, &[body_id]);
        mutator.append_children(root, &[html]);
        drop(mutator);
        doc.resolve(0.0);
        render_to_buffer::<VelloCpuImageRenderer, _>(|scene| paint_on_canvas(scene, &mut doc), W, H)
    }

    /// Chromium ignores `mix-blend-mode` on the root element; a blended body
    /// blends with the canvas, which the root's background paints.
    #[test]
    fn root_and_body_match_chromium() {
        let buffer = render(
            "",
            &format!("background:{B};mix-blend-mode:multiply"),
            vec![fill(&format!("background:{S}"))],
        );
        assert_near(pixel(&buffer, 10, 10), UNBLENDED, "blended root");
        let buffer = render(
            "body { mix-blend-mode: multiply }",
            &format!("background:{B}"),
            vec![fill(&format!("background:{S}"))],
        );
        assert_near(pixel(&buffer, 10, 10), MULTIPLIED, "blended body");
        assert_near(pixel(&buffer, 100, 80), [40, 120, 200], "canvas");
    }

    #[test]
    fn background_blend_mode_matches_chromium() {
        let gradient = format!("linear-gradient({S}, {S})");
        let body = vec![
            abs(
                10,
                10,
                &format!(
                    "background-color:{B};background-image:{gradient};background-blend-mode:multiply"
                ),
                vec![],
            ),
            // Without a colour the layer blends with nothing: the backdrop
            // behind the element is not part of the group.
            abs(60, 10, &format!("background:{B}"), vec![]),
            abs(
                60,
                10,
                &format!("background-image:{gradient};background-blend-mode:multiply"),
                vec![],
            ),
        ];
        let buffer = render("", "", body);
        assert_near(pixel(&buffer, 30, 30), [34, 42, 47], "over its colour");
        assert_near(pixel(&buffer, 80, 30), UNBLENDED, "without a colour");
    }
}
