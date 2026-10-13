use std::sync::{Arc, Mutex};

use crate::stylo_to_parley;
use app_units::Au;
use parley::{FontContext, FontVariation};
use skrifa::MetadataProvider as _;
use skrifa::charmap::Charmap;
use style::properties::style_structs::Font as FontStyles;
use style::{
    device::servo::FontMetricsProvider,
    font_metrics::FontMetrics,
    values::computed::{CSSPixelLength, font::QueryFontMetricsFlags},
};

/// Metrics Blink uses for the intrinsic inline size of a text control.
///
/// The average comes from the primary font's OS/2 table when it is sensible;
/// otherwise the shaped `0` advance is the compatibility fallback. `max_width`
/// is only meaningful alongside a valid average width.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TextControlFontMetrics {
    pub(crate) average_width: f32,
    pub(crate) max_width: Option<f32>,
}

/// Query fontique for the fonts matching `font_styles` (family list and attributes).
fn query_for_font_styles<'a>(
    font_ctx: &'a mut FontContext,
    font_styles: &'a FontStyles,
) -> parley::fontique::Query<'a> {
    use parley::fontique::Attributes;

    let mut query = font_ctx.collection.query(&mut font_ctx.source_cache);
    let families = font_styles
        .font_family
        .families
        .iter()
        .map(stylo_to_parley::query_font_family);
    query.set_families(families);
    query.set_attributes(Attributes {
        width: stylo_to_parley::font_width(font_styles.font_width),
        weight: stylo_to_parley::font_weight(font_styles.font_weight),
        style: stylo_to_parley::font_style(font_styles.font_style),
    });
    query
}

/// Return primary-font metrics for text-control intrinsic sizing.
///
/// Chromium on Linux keeps a valid OS/2 `xAvgCharWidth` at its fractional
/// value and uses the rounded horizontal extent of the font bounds as the
/// control's maximum character width. It falls back to the `0` glyph when the
/// average table is missing or implausibly wide. This keeps a CSS input's
/// automatic minimum separate from the `ch` unit, which is always based on one
/// glyph advance.
pub(crate) fn text_control_font_metrics(
    font_ctx: &mut FontContext,
    font_styles: &FontStyles,
    font_size: f32,
) -> Option<TextControlFontMetrics> {
    use skrifa::instance::{LocationRef, Size};
    use skrifa::metrics::{GlyphMetrics, Metrics};

    // The primary font is the first available font: the first face whose
    // `unicode-range` includes U+0020 (CSS Fonts 4; Chromium's
    // FontFallbackList::DeterminePrimarySimpleFontData). A family of
    // fontsource subsets declares its cyrillic face, which has no `0`, first.
    let primary = query_for_font_styles(font_ctx, font_styles).first_available_font()?;
    let font_ref = skrifa::FontRef::from_index(primary.blob.as_ref(), primary.index).ok()?;
    let variations = stylo_to_parley::font_variations(&font_styles.font_variation_settings);
    let location = font_ref.axes().location(
        variations
            .iter()
            .map(|v| (skrifa::Tag::from_be_bytes(v.tag.to_bytes()), v.value)),
    );
    let location_ref = LocationRef::from(&location);
    let metrics = Metrics::new(&font_ref, Size::new(font_size), location_ref);
    let zero_width = {
        let glyph_id = Charmap::new(&font_ref).map('0')?;
        GlyphMetrics::new(&font_ref, Size::new(font_size), location_ref).advance_width(glyph_id)?
    };
    let average_width = metrics
        .average_width
        .filter(|width| width.is_finite() && *width > 0.0 && *width <= zero_width * 1.7);
    Some(match average_width {
        Some(average_width) => TextControlFontMetrics {
            average_width,
            // Blink's Linux SimpleFontData uses
            // round(SkFontMetrics::fXMax - fXMin), rather than hhea's
            // advanceWidthMax. Skrifa exposes the equivalent scaled head
            // bounds through `Metrics::bounds`.
            max_width: metrics
                .bounds
                .map(|bounds| (bounds.x_max - bounds.x_min).round())
                .filter(|width| width.is_finite() && *width > 0.0),
        },
        None => TextControlFontMetrics {
            average_width: zero_width,
            max_width: None,
        },
    })
}

#[derive(Clone)]
pub(crate) struct BlitzFontMetricsProvider {
    pub(crate) font_ctx: Arc<Mutex<FontContext>>,
}

impl core::fmt::Debug for BlitzFontMetricsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BlitzFontMetricsProvider")
    }
}

impl FontMetricsProvider for BlitzFontMetricsProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        font_styles: &FontStyles,
        font_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        use parley::fontique::{Query, QueryFont, QueryStatus};
        use skrifa::instance::{LocationRef, Size};
        use skrifa::metrics::{GlyphMetrics, Metrics};

        // Lock font_ctx. Explicit reborrow required for borrow checker.
        let mut font_ctx = self.font_ctx.lock().unwrap();
        let font_ctx = &mut *font_ctx;

        // Query fontique for the font that matches the font styles
        let mut query = query_for_font_styles(font_ctx, font_styles);
        // let fb_script = crate::swash_convert::script_to_fontique(script);
        // let fb_language = locale.and_then(crate::swash_convert::locale_to_fontique);
        // query.set_fallbacks(fontique::FallbackKey::new(fb_script, fb_language.as_ref()));

        let variations = stylo_to_parley::font_variations(&font_styles.font_variation_settings);
        // let features = self.rcx.features(style.font_features).unwrap_or(&[]);

        // fn name_of(font_ref: &skrifa::FontRef) -> String {
        //     use skrifa::string::StringId;
        //     font_ref
        //         .localized_strings(StringId::POSTSCRIPT_NAME)
        //         .english_or_first()
        //         .unwrap()
        //         .chars()
        //         .collect()
        // }

        fn find_font_for(query: &mut Query, ch: char) -> Option<QueryFont> {
            let mut font = None;
            query.matches_with(|q_font: &QueryFont| {
                // Fontique's character map of a query font maps only the
                // characters in the face's `unicode-range`.
                if q_font
                    .charmap()
                    .is_some_and(|charmap| charmap.map(ch).is_some())
                {
                    font = Some(q_font.clone());
                    QueryStatus::Stop
                } else {
                    QueryStatus::Continue
                }
            });
            font
        }

        /// Scales the advance the same way as Parley's shaped glyph advances (font units × size / upem)
        /// rather than with skrifa's FreeType-compatible fixed-point scale, so that `ch`/`ic`
        /// lengths exactly match the width of the corresponding laid-out text.
        fn advance_of(
            query: &mut Query,
            ch: char,
            font_size: f32,
            variations: &[FontVariation],
        ) -> Option<f32> {
            let font = find_font_for(query, ch)?;
            let font_ref = skrifa::FontRef::from_index(font.blob.as_ref(), font.index).ok()?;
            let location = font_ref.axes().location(
                variations
                    .iter()
                    .map(|v| (skrifa::Tag::from_be_bytes(v.tag.to_bytes()), v.value)),
            );
            let location_ref = LocationRef::from(&location);
            let upem = Metrics::new(&font_ref, Size::unscaled(), location_ref).units_per_em;
            if upem == 0 {
                return None;
            }
            let glyph_metrics = GlyphMetrics::new(&font_ref, Size::unscaled(), location_ref);
            let char_map = Charmap::new(&font_ref);
            let glyph_id = char_map.map(ch)?;
            let advance = glyph_metrics.advance_width(glyph_id)?;
            Some(advance * (font_size / upem as f32))
        }

        fn metrics_of(
            query: &mut Query,
            ch: char,
            font_size: Size,
            variations: &[FontVariation],
        ) -> Option<(f32, Option<f32>, Option<f32>)> {
            let font = find_font_for(query, ch)?;
            let font_ref = skrifa::FontRef::from_index(font.blob.as_ref(), font.index).ok()?;
            let location = font_ref.axes().location(
                variations
                    .iter()
                    .map(|v| (skrifa::Tag::from_be_bytes(v.tag.to_bytes()), v.value)),
            );
            let location_ref = LocationRef::from(&location);
            let metrics = Metrics::new(&font_ref, font_size, location_ref);
            Some((metrics.ascent, metrics.x_height, metrics.cap_height))
        }

        let zero_advance = advance_of(&mut query, '0', font_size.px(), &variations);
        let ic_advance = advance_of(&mut query, '\u{6C34}', font_size.px(), &variations);
        let (ascent, x_height, cap_height) =
            metrics_of(&mut query, ' ', Size::new(font_size.px()), &variations)
                .unwrap_or((0.0, None, None));

        FontMetrics {
            ascent: CSSPixelLength::new(ascent),
            x_height: x_height.filter(|xh| *xh != 0.0).map(CSSPixelLength::new),
            cap_height: cap_height.map(CSSPixelLength::new),
            zero_advance_measure: zero_advance.map(CSSPixelLength::new),
            ic_width: ic_advance.map(CSSPixelLength::new),
            script_percent_scale_down: None,
            script_script_percent_scale_down: None,
        }
    }

    fn base_size_for_generic(
        &self,
        generic: style::values::computed::font::GenericFontFamily,
    ) -> style::values::computed::Length {
        let size = match generic {
            style::values::computed::font::GenericFontFamily::Monospace => 13.0,
            _ => 16.0,
        };
        style::values::computed::Length::from(Au::from_f32_px(size))
    }
}

#[cfg(test)]
mod unicode_range_tests {
    //! Families of `@font-face` rules split by `unicode-range`, built from
    //! DejaVu Sans and the crate's bullet font (space and bullets, no digits),
    //! against Chromium 153's widths for the same rules:
    //!
    //! - Primary: bullet font for U+0400-04FF, DejaVu Sans for U+0000-00FF,
    //!   then the bullet font for U+2190-21FF.
    //! - Order: bullet font, then DejaVu Sans, both for U+2000-206F.
    //! - Range: DejaVu Sans without a range, then the bullet font for U+0000-00FF.
    //! - Loaded: the rules of Order, registered in the other order (as their
    //!   sources finish loading); Chromium orders the faces by their rules.

    use blitz_traits::shell::{ColorScheme, Viewport};

    use crate::net::{FontFaceOverrides, Resource, ResourceLoadResponse};
    use crate::{Attribute, BaseDocument, DocumentConfig, NodeId, qual_name};

    const DEJAVU: &[u8] = include_bytes!("../../../examples/wasm_hello/assets/DejaVuSans.woff2");

    fn attrs(pairs: &[(&str, &str)]) -> Vec<Attribute> {
        pairs
            .iter()
            .map(|(name, value)| Attribute {
                name: match *name {
                    "size" => qual_name!("size"),
                    _ => qual_name!("style"),
                },
                value: (*value).into(),
            })
            .collect()
    }

    /// Register a face as an `@font-face` rule with `family` and `range`
    /// would, in definition order.
    fn face(doc: &mut BaseDocument, family: &str, bytes: &[u8], range: Option<(u32, u32)>) {
        loaded_face(doc, family, bytes, range, None);
    }

    /// Register a face as `face` does, with the position of its rule among
    /// the `@font-face` rules (`order`), as `fetch_font_face_rules` gives it.
    fn loaded_face(
        doc: &mut BaseDocument,
        family: &str,
        bytes: &[u8],
        range: Option<(u32, u32)>,
        order: Option<u64>,
    ) {
        doc.load_resource(ResourceLoadResponse {
            request_id: 0,
            node_id: None,
            resolved_url: Some(format!("test://{family}")),
            result: Ok(Resource::Font(
                blitz_traits::net::Bytes::from(crate::decode_font_bytes(bytes).into_owned()),
                FontFaceOverrides {
                    family_name: Some(family.into()),
                    // `fetch_font_face_rules` gives a rule without the
                    // descriptor the whole range.
                    unicode_range: Some(vec![range.unwrap_or((0, 0x10FFFF))]),
                    order,
                    ..Default::default()
                },
            )),
        });
    }

    /// An element: its name, attributes and text.
    type Element<'a> = (&'a str, &'a [(&'a str, &'a str)], Option<&'a str>);

    /// The widths of `elements`, each in a block, in an 800x600 document.
    fn widths(elements: &[Element]) -> Vec<f32> {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            font_ctx: Some(crate::build_single_font_ctx(DEJAVU)),
            ..Default::default()
        });
        let bullet = crate::BULLET_FONT;
        face(&mut doc, "PlainDejaVu", DEJAVU, None);
        face(&mut doc, "Primary", bullet, Some((0x400, 0x4FF)));
        face(&mut doc, "Primary", DEJAVU, Some((0, 0xFF)));
        face(&mut doc, "Primary", bullet, Some((0x2190, 0x21FF)));
        face(&mut doc, "Order", bullet, Some((0x2000, 0x206F)));
        face(&mut doc, "Order", DEJAVU, Some((0x2000, 0x206F)));
        face(&mut doc, "Range", DEJAVU, None);
        face(&mut doc, "Range", bullet, Some((0, 0xFF)));
        // The rules of Order, their sources loaded in the other order.
        loaded_face(&mut doc, "Loaded", DEJAVU, Some((0x2000, 0x206F)), Some(1));
        loaded_face(&mut doc, "Loaded", bullet, Some((0x2000, 0x206F)), Some(0));

        let root = doc.root_node().id;
        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html", html), vec![]);
        let body =
            mutator.create_element(qual_name!("body", html), attrs(&[("style", "margin:0")]));
        let mut ids: Vec<NodeId> = Vec::new();
        for (name, attributes, text) in elements {
            let element = match *name {
                "input" => qual_name!("input", html),
                _ => qual_name!("span", html),
            };
            let id = mutator.create_element(element, attrs(attributes));
            if let Some(text) = text {
                let text = mutator.create_text_node(text);
                mutator.append_children(id, &[text]);
            }
            let block = mutator.create_element(qual_name!("div", html), vec![]);
            mutator.append_children(block, &[id]);
            mutator.append_children(body, &[block]);
            ids.push(id);
        }
        mutator.append_children(html, &[body]);
        mutator.append_children(root, &[html]);
        drop(mutator);
        doc.resolve(0.0);
        ids.into_iter()
            .map(|id| doc.nodes[id].unrounded_layout().size.width)
            .collect()
    }

    /// A text control's primary font is the first face, latest defined first,
    /// whose range includes U+0020: DejaVu Sans, not either bullet face.
    /// Chromium gives the same widths as for DejaVu Sans alone.
    #[test]
    fn text_control_uses_the_first_face_covering_space() {
        const INPUT: &str = "padding:0;border:0;font-size:16px;font-family:";
        let primary = format!("{INPUT}Primary");
        let plain = format!("{INPUT}PlainDejaVu");
        for (style, expected) in [(primary, [200.0, 45.0, 54.0]), (plain, [200.0, 45.0, 54.0])] {
            assert_eq!(
                widths(&[
                    ("input", &[("style", &style)], None),
                    ("input", &[("style", &style), ("size", "1")], None),
                    ("input", &[("style", &style), ("size", "2")], None),
                ]),
                expected,
                "{style}"
            );
        }
    }

    /// A character takes the latest-defined face whose range includes it, and
    /// a face whose range excludes it is no candidate even if it has the
    /// glyph. Chromium sets "•••" in DejaVu Sans (35.390625 px at 20px; the
    /// bullet font's are 33.640625 px) in both families, and in Loaded, whose
    /// faces were registered out of their rules' order.
    #[test]
    fn characters_take_the_latest_face_whose_range_includes_them() {
        const SPAN: &str = "display:inline-block;font-size:20px;font-family:";
        let bullets = Some("\u{2022}\u{2022}\u{2022}");
        assert_eq!(
            widths(&[
                ("span", &[("style", &format!("{SPAN}Order"))], bullets),
                ("span", &[("style", &format!("{SPAN}Range"))], bullets),
                ("span", &[("style", &format!("{SPAN}PlainDejaVu"))], bullets),
                ("span", &[("style", &format!("{SPAN}Loaded"))], bullets),
            ]),
            [35.390625, 35.390625, 35.390625, 35.390625]
        );
    }
}
