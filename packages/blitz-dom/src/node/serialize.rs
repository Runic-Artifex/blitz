//! Serialization of nodes to HTML, and of inline `<svg>` subtrees to SVG source for usvg.

use html_escape::{encode_quoted_attribute_to_string, encode_text_to_string};
use markup5ever::{QualName, ns};

use super::{Node, NodeData};

#[derive(Clone, Copy)]
enum OutputStyle {
    Normal,
    Pretty,
}

/// Writes an attribute's serialized name, per the HTML fragment serialization algorithm
fn write_attr_name(name: &QualName, writer: &mut String) {
    match name.ns {
        ns!() | ns!(html) => {}
        ns!(xml) => writer.push_str("xml:"),
        ns!(xmlns) if name.local.as_str() == "xmlns" => {}
        ns!(xmlns) => writer.push_str("xmlns:"),
        ns!(xlink) => writer.push_str("xlink:"),
        _ => {
            if let Some(prefix) = &name.prefix {
                writer.push_str(prefix);
                writer.push(':');
            }
        }
    }
    writer.push_str(&name.local);
}

impl Node {
    /// Renders the HTML of this node and all its children as a `String` without extra whitespace.
    ///
    /// Example output:
    ///
    /// ```text
    /// <html><head /><body><main id="main"><div class="arbitrary-class" /></main></body></html>
    /// ```
    pub fn outer_html(&self) -> String {
        let mut output = String::new();
        self.write_outer_html(&mut output);
        output
    }

    /// Renders the HTML of this node and all its children as a `String` with whitespace for human
    /// readability.
    ///
    /// Example output:
    ///
    /// ```text
    /// <html>
    ///   <head />
    ///   <body>
    ///     <main id="main">
    ///       <div class="arbitrary-class" />
    ///     </main>
    ///   </body>
    /// </html>
    /// ```
    pub fn outer_html_pretty(&self) -> String {
        let mut output = String::new();
        self.write_outer_html_pretty(&mut output);
        output
    }

    pub fn write_outer_html(&self, writer: &mut String) {
        self.write_outer_html_in_style(writer, OutputStyle::Normal, 0);
    }

    pub fn write_outer_html_pretty(&self, writer: &mut String) {
        self.write_outer_html_in_style(writer, OutputStyle::Pretty, 0);
    }

    fn write_outer_html_in_style(&self, writer: &mut String, style: OutputStyle, nesting: usize) {
        const INDENT: &str = "  ";
        let has_children = !self.children.is_empty();

        match &self.data {
            NodeData::Document(_) => {}
            NodeData::Comment { .. } => {}
            NodeData::AnonymousBlock(_) => {}
            // NodeData::Doctype { name, .. } => write!(s, "DOCTYPE {name}"),
            NodeData::Text(data) => {
                if matches!(style, OutputStyle::Pretty) {
                    for _ in 0..nesting {
                        writer.push_str(INDENT);
                    }
                }
                let in_raw_text_element = self
                    .parent
                    .and_then(|id| self.tree()[id].data.downcast_element())
                    .is_some_and(|el| {
                        // Documents are parsed with scripting disabled, so `<noscript>` is not raw text
                        el.name.ns == ns!(html)
                            && matches!(
                                el.name.local.as_ref(),
                                "style"
                                    | "script"
                                    | "xmp"
                                    | "iframe"
                                    | "noembed"
                                    | "noframes"
                                    | "plaintext"
                            )
                    });
                if in_raw_text_element {
                    writer.push_str(data.content.as_str());
                } else {
                    encode_text_to_string(data.content.as_str(), writer);
                }
                if matches!(style, OutputStyle::Pretty) {
                    writer.push('\n');
                }
            }
            NodeData::Element(data) => {
                if matches!(style, OutputStyle::Pretty) {
                    for _ in 0..nesting {
                        writer.push_str(INDENT);
                    }
                }
                writer.push('<');
                writer.push_str(&data.name.local);

                for attr in data.attrs() {
                    writer.push(' ');
                    write_attr_name(&attr.name, writer);
                    writer.push_str("=\"");
                    encode_quoted_attribute_to_string(&attr.value, writer);
                    writer.push('"');
                }
                if !has_children {
                    writer.push_str(" /");
                }
                writer.push('>');
                if matches!(style, OutputStyle::Pretty) {
                    writer.push('\n');
                }

                if has_children {
                    for &child_id in &self.children {
                        self.tree()[child_id].write_outer_html_in_style(writer, style, nesting + 1);
                    }

                    if matches!(style, OutputStyle::Pretty) {
                        for _ in 0..nesting {
                            writer.push_str(INDENT);
                        }
                    }
                    writer.push_str("</");
                    writer.push_str(&data.name.local);
                    writer.push('>');
                    if matches!(style, OutputStyle::Pretty) {
                        writer.push('\n');
                    }
                }
            }
        }
    }
}

#[cfg(feature = "svg")]
mod svg {
    //! The SVG source usvg draws an inline `<svg>` from.
    //!
    //! Every element is written with its *computed* SVG painting properties,
    //! so stylesheet rules, inline styles and inherited values reach usvg as
    //! they reach a browser's SVG renderer: the presentation attributes are
    //! part of the cascade (`push_svg_presentation_hints` in `stylo.rs`) and
    //! are replaced here by the values they compute to. `currentColor` is
    //! resolved with each element's own `color`, as CSS resolves it after
    //! inheritance. Children with `display: none` are left out; `opacity`,
    //! `visibility` and the transform properties of children are written as
    //! computed (the outermost `<svg>`'s own are applied to its box by
    //! blitz-paint).

    use html_escape::{encode_quoted_attribute_to_string, encode_text_to_string};
    use markup5ever::{QualName, ns};
    use style::computed_values::visibility::T as Visibility;
    use style::properties::ComputedValues;
    use style::values::computed::svg::{SVGOpacity, SVGPaint, SVGPaintKind};
    use style::values::computed::{
        CSSPixelLength, Color, LengthPercentage, NonNegativeLengthPercentage,
    };
    use style::values::generics::svg::{GenericSVGLength, SVGPaintFallback, SVGStrokeDashArray};
    use style_traits::values::ToCss;

    use super::{Node, NodeData, write_attr_name};
    use crate::util::ToColorColor as _;

    const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

    /// The inherited properties written for every element whose value differs
    /// from its parent's, in this order.
    const INHERITED: [&str; 12] = [
        "fill",
        "fill-opacity",
        "fill-rule",
        "stroke",
        "stroke-width",
        "stroke-opacity",
        "stroke-linecap",
        "stroke-linejoin",
        "stroke-miterlimit",
        "stroke-dasharray",
        "stroke-dashoffset",
        "visibility",
    ];

    /// Attributes whose values are taken from the computed style instead:
    /// the presentation attributes mapped into the cascade, and `style`,
    /// which usvg would otherwise apply over the computed values.
    fn is_computed_attr(name: &QualName, is_root: bool) -> bool {
        let local: &str = &name.local;
        name.ns == ns!()
            && (INHERITED.contains(&local)
                || matches!(local, "opacity" | "color" | "display" | "style")
                || (!is_root && local == "transform"))
    }

    type Inherited = [Option<String>; INHERITED.len()];

    /// `rgba()` in sRGB: usvg parses sRGB colours only, so `oklch()`,
    /// `color-mix()` results and other spaces are converted as painting
    /// converts them.
    fn rgba(color: &Color, style: &ComputedValues) -> String {
        let rgba = color
            .resolve_to_absolute(&style.clone_color())
            .as_color_color()
            .to_rgba8();
        format!(
            "rgba({}, {}, {}, {})",
            rgba.r,
            rgba.g,
            rgba.b,
            f32::from(rgba.a) / 255.0
        )
    }

    fn paint(paint: &SVGPaint, style: &ComputedValues) -> Option<String> {
        let mut out = match &paint.kind {
            SVGPaintKind::None => "none".to_string(),
            SVGPaintKind::Color(color) => rgba(color, style),
            SVGPaintKind::PaintServer(url) => {
                // Presentation attributes resolve against `about:blank`;
                // usvg looks paint servers up by fragment within the source.
                let fragment = url
                    .url()
                    .and_then(|url| url.fragment().map(str::to_owned))?;
                format!("url(#{fragment})")
            }
            SVGPaintKind::ContextFill => "context-fill".to_string(),
            SVGPaintKind::ContextStroke => "context-stroke".to_string(),
        };
        if matches!(paint.kind, SVGPaintKind::PaintServer(_)) {
            match &paint.fallback {
                SVGPaintFallback::None => out.push_str(" none"),
                SVGPaintFallback::Color(color) => {
                    out.push(' ');
                    out.push_str(&rgba(color, style));
                }
                SVGPaintFallback::Unset => {}
            }
        }
        Some(out)
    }

    fn length(value: &LengthPercentage) -> String {
        if let Some(length) = value.to_length() {
            length.px().to_string()
        } else if let Some(percentage) = value.to_percentage() {
            format!("{}%", percentage.0 * 100.0)
        } else {
            value.to_css_string()
        }
    }

    fn svg_length<L>(
        value: &GenericSVGLength<L>,
        to_lp: impl Fn(&L) -> &LengthPercentage,
    ) -> Option<String> {
        match value {
            GenericSVGLength::LengthPercentage(lp) => Some(length(to_lp(lp))),
            GenericSVGLength::ContextValue => None,
        }
    }

    fn opacity(value: &SVGOpacity) -> Option<String> {
        match value {
            SVGOpacity::Opacity(value) => Some(value.to_string()),
            _ => None,
        }
    }

    fn inherited(style: &ComputedValues) -> Inherited {
        let svg = style.get_inherited_svg();
        let dasharray = match &svg.stroke_dasharray {
            SVGStrokeDashArray::Values(values) if values.is_empty() => Some("none".to_string()),
            SVGStrokeDashArray::Values(values) => Some(
                values
                    .iter()
                    .map(|value: &NonNegativeLengthPercentage| length(&value.0))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            SVGStrokeDashArray::ContextValue => None,
        };
        let visibility = match style.get_inherited_box().visibility {
            Visibility::Visible => "visible",
            Visibility::Hidden | Visibility::Collapse => "hidden",
        };
        [
            paint(&svg.fill, style),
            opacity(&svg.fill_opacity),
            Some(svg.fill_rule.to_css_string()),
            paint(&svg.stroke, style),
            svg_length(&svg.stroke_width, |lp: &NonNegativeLengthPercentage| &lp.0),
            opacity(&svg.stroke_opacity),
            Some(svg.stroke_linecap.to_css_string()),
            Some(svg.stroke_linejoin.to_css_string()),
            Some(svg.stroke_miterlimit.to_css_string()),
            dasharray,
            svg_length(&svg.stroke_dashoffset, |lp: &LengthPercentage| lp),
            Some(visibility.to_string()),
        ]
    }

    fn write_attr(writer: &mut String, name: &str, value: &str) {
        writer.push(' ');
        writer.push_str(name);
        writer.push_str("=\"");
        encode_quoted_attribute_to_string(value, writer);
        writer.push('"');
    }

    impl Node {
        /// Serializes this `<svg>` subtree as a standalone SVG document for
        /// usvg, with each element's computed SVG painting properties.
        pub(crate) fn svg_source(&self) -> String {
            // The reference box of transforms in the SVG content
            // (`transform-box: view-box`, the only value Stylo supports).
            let view_box = self
                .attr(markup5ever::local_name!("viewBox"))
                .and_then(|value| value.parse::<svgtypes::ViewBox>().ok())
                .map(|view_box| (view_box.w as f32, view_box.h as f32))
                .unwrap_or_else(|| {
                    let size = self.final_layout().size;
                    (size.width, size.height)
                });
            let mut output = String::new();
            self.write_svg(&mut output, None, view_box, true);
            output
        }

        fn write_svg(
            &self,
            writer: &mut String,
            parent: Option<&Inherited>,
            view_box: (f32, f32),
            is_root: bool,
        ) {
            match &self.data {
                NodeData::Document(_) | NodeData::Comment { .. } | NodeData::AnonymousBlock(_) => {}
                NodeData::Text(data) => {
                    encode_text_to_string(data.content.as_str(), writer);
                }
                NodeData::Element(data) => {
                    let style = self.primary_styles();
                    let style = style.as_deref();
                    if !is_root && style.is_some_and(|style| style.get_box().display.is_none()) {
                        return;
                    }

                    writer.push('<');
                    writer.push_str(&data.name.local);
                    if is_root
                        && !data.attrs().iter().any(|attr| {
                            (attr.name.ns == ns!(xmlns) && &*attr.name.local == "xlink")
                                || &*attr.name.local == "xmlns:xlink"
                        })
                    {
                        write_attr(writer, "xmlns:xlink", XLINK_NS);
                    }

                    let current_color = style.map(|style| rgba(&Color::CurrentColor, style));
                    for attr in data.attrs() {
                        if style.is_some() && is_computed_attr(&attr.name, is_root) {
                            continue;
                        }
                        writer.push(' ');
                        write_attr_name(&attr.name, writer);
                        writer.push_str("=\"");
                        // Other colour attributes (`stop-color`, `flood-color`, …)
                        // are not in the cascade: resolve their `currentColor` here.
                        match &current_color {
                            Some(color) if attr.value.contains("currentColor") => {
                                let value = attr.value.replace("currentColor", color);
                                encode_quoted_attribute_to_string(&value, writer);
                            }
                            _ => {
                                encode_quoted_attribute_to_string(&attr.value, writer);
                            }
                        }
                        writer.push('"');
                    }

                    let own = style.map(|style| inherited(style));
                    if let (Some(style), Some(own)) = (style, &own) {
                        for (index, value) in own.iter().enumerate() {
                            let Some(value) = value else { continue };
                            if parent.is_some_and(|parent| parent[index].as_ref() == Some(value)) {
                                continue;
                            }
                            write_attr(writer, INHERITED[index], value);
                        }
                        if !is_root {
                            let opacity = style.get_effects().opacity;
                            if opacity != 1.0 {
                                write_attr(writer, "opacity", &opacity.to_string());
                            }
                            let reference_box = euclid::default::Rect::new(
                                euclid::default::Point2D::new(
                                    CSSPixelLength::new(0.0),
                                    CSSPixelLength::new(0.0),
                                ),
                                euclid::default::Size2D::new(
                                    CSSPixelLength::new(view_box.0),
                                    CSSPixelLength::new(view_box.1),
                                ),
                            );
                            if let Some(transform) = crate::stylo_to_kurbo::resolve_2d_transform(
                                style.get_box(),
                                reference_box,
                            ) {
                                let [a, b, c, d, e, f] = transform.as_coeffs();
                                write_attr(
                                    writer,
                                    "transform",
                                    &format!("matrix({a} {b} {c} {d} {e} {f})"),
                                );
                            }
                        }
                    }

                    if self.children.is_empty() {
                        writer.push_str(" />");
                    } else {
                        writer.push('>');
                        let parent = own.as_ref().or(parent);
                        for &child_id in &self.children {
                            self.tree()[child_id].write_svg(writer, parent, view_box, false);
                        }
                        writer.push_str("</");
                        writer.push_str(&data.name.local);
                        writer.push('>');
                    }
                }
            }
        }
    }
}
