//! CSS text paint phases belong to inline fragments, independently of glyph hinting.

use blitz_dom::{BaseDocument, Node, NodeId, node::TextBrush};
use parley::{Layout, Line};
use std::collections::HashMap;
use style::Zero;
use style::values::computed::{
    AlignmentBaseline, BaselineShift, BorderStyle, Image, Length, OutlineStyle,
};

use crate::color::ToColorColor;

struct Span {
    parent: Option<usize>,
    materialized: bool,
    inset: f32,
}

#[derive(Default)]
pub(crate) struct TextOrigins {
    spans: Vec<Span>,
    indices: HashMap<NodeId, usize>,
    origin_y: f64,
    paint_y: f64,
}

impl TextOrigins {
    pub(crate) fn prepare(
        &mut self,
        doc: &BaseDocument,
        root_id: NodeId,
        layout: &Layout<TextBrush>,
    ) -> bool {
        self.spans.clear();
        self.indices.clear();
        self.prepare_inner(doc, root_id, layout).is_some()
    }

    fn prepare_inner(
        &mut self,
        doc: &BaseDocument,
        root_id: NodeId,
        layout: &Layout<TextBrush>,
    ) -> Option<()> {
        let root = doc.get_node(root_id)?;
        let inline = &root.element_data()?.inline_layout_data.as_ref()?.layout;
        // Editors, placeholders and markers have separate layouts and paint origins.
        if !std::ptr::eq(inline, layout) {
            return None;
        }
        let styles = layout.styles();
        let line = layout.lines().next()?;
        let box_layout = root.unrounded_layout();
        let width = box_layout.content_box_width();
        let origin = root.unrounded_absolute_position(0.0, 0.0);
        let painted = root.absolute_position(0.0, 0.0);
        let painted_layout = root.final_layout();
        let scroll = doc.viewport_scroll().y;
        // CSS anchors use the unrounded layout position. The glyph transform already
        // contains the rounded paint position, so compensate for that separately.
        let origin_y =
            f64::from(origin.y + box_layout.border.top + box_layout.padding.top) - scroll;
        let paint_y =
            f64::from(painted.y + painted_layout.border.top + painted_layout.padding.top) - scroll;
        for (index, style) in styles.iter().enumerate() {
            // DOM style lookup also selects the first style for a node. Text source
            // normalization can give one node more than one style entry.
            self.indices.entry(style.brush.id).or_insert(index);
        }
        let spans = &mut self.spans;
        spans.reserve(styles.len());
        for style in styles {
            let node = doc.get_node(style.brush.id)?;
            let mut parent_id = node.parent;
            let parent = loop {
                let Some(id) = parent_id else { break None };
                if let Some(&index) = self.indices.get(&id) {
                    break Some(index);
                }
                parent_id = doc.get_node(id).and_then(|n| n.parent);
            };
            let computed = node.primary_styles()?;
            let border_top = if computed.clone_border_top_style().none_or_hidden() {
                0.0
            } else {
                computed.clone_border_top_width().0.to_f32_px()
            };
            let inset = computed
                .clone_padding_top()
                .0
                .resolve(Length::new(width))
                .px()
                + border_top;
            spans.push(Span {
                parent,
                materialized: node.id != root_id && initially_materialized(node),
                inset,
            });
        }
        // A shifted/differently-sized nested inline must retain its parent's own box.
        // A leaf's vertical alignment alone does not materialize the leaf itself.
        for (index, style) in styles.iter().enumerate() {
            let Some(parent) = spans[index].parent else {
                continue;
            };
            if styles[parent].brush.id == root_id {
                continue;
            }
            let node = doc.get_node(style.brush.id)?;
            let computed = node.primary_styles()?;
            let box_style = computed.get_box();
            let aligned = box_style.clone_alignment_baseline() != AlignmentBaseline::Baseline
                || !geometric_baseline_shift_is_zero(box_style.clone_baseline_shift());
            if has_margin(node)
                || aligned
                || line.style_font_metrics(index) != line.style_font_metrics(parent)
            {
                spans[parent].materialized = true;
            }
        }
        self.origin_y = origin_y;
        self.paint_y = paint_y;
        Some(())
    }

    pub(crate) fn offset(&self, line: Line<'_, TextBrush>, style: usize, scale: f64) -> f32 {
        let mut index = Some(style);
        let mut top = f64::from(line.metrics().block_min_coord);
        while let Some(current) = index {
            let Some(span) = self.spans.get(current) else {
                break;
            };
            if span.materialized {
                if let Some((content_top, _)) = line.style_content_bounds(current) {
                    top = f64::from(content_top) - f64::from(span.inset) * scale;
                }
                break;
            }
            index = span.parent;
        }
        let anchor = top / scale;
        paint_shift(self.origin_y, self.paint_y, anchor, scale)
    }
}

fn geometric_baseline_shift_is_zero(shift: BaselineShift) -> bool {
    match shift {
        BaselineShift::ImplicitZero => true,
        BaselineShift::Length(value) => value.is_zero(),
        BaselineShift::Keyword(_) => false,
    }
}

fn paint_shift(origin: f64, painted: f64, anchor: f64, scale: f64) -> f32 {
    (((origin + anchor + 0.5).floor() - (painted + anchor)) * scale) as f32
}

fn has_margin(node: &Node) -> bool {
    let Some(style) = node.primary_styles() else {
        return false;
    };
    ![
        style.clone_margin_top(),
        style.clone_margin_right(),
        style.clone_margin_bottom(),
        style.clone_margin_left(),
    ]
    .iter()
    .all(Zero::is_zero)
}

fn initially_materialized(node: &Node) -> bool {
    let Some(style) = node.primary_styles() else {
        return false;
    };
    let background = style
        .clone_background_color()
        .resolve_to_absolute(&style.clone_color())
        .as_srgb_color();
    background.components[3] != 0.0
        || has_margin(node)
        || style
            .get_background()
            .background_image
            .0
            .iter()
            .any(|image| !matches!(image, Image::None))
        || !style.get_effects().box_shadow.0.is_empty()
        || (style.get_outline().outline_width.0.to_f32_px() != 0.0
            && !matches!(
                style.get_outline().outline_style,
                OutlineStyle::BorderStyle(BorderStyle::None | BorderStyle::Hidden)
            ))
        || node.element_data().is_some_and(|element| {
            <_ as AsRef<str>>::as_ref(&element.name.local) == "a"
                && node.attr("href".into()).is_some()
        })
        || ![
            style.clone_padding_top(),
            style.clone_padding_right(),
            style.clone_padding_bottom(),
            style.clone_padding_left(),
        ]
        .iter()
        .all(Zero::is_zero)
        || [
            (
                style.clone_border_top_style(),
                style.clone_border_top_width().0.to_f32_px(),
            ),
            (
                style.clone_border_right_style(),
                style.clone_border_right_width().0.to_f32_px(),
            ),
            (
                style.clone_border_bottom_style(),
                style.clone_border_bottom_width().0.to_f32_px(),
            ),
            (
                style.clone_border_left_style(),
                style.clone_border_left_width().0.to_f32_px(),
            ),
        ]
        .iter()
        .any(|(border, width)| !border.none_or_hidden() && *width != 0.0)
}

#[cfg(test)]
mod tests {
    use super::{geometric_baseline_shift_is_zero, paint_shift};
    use style::values::computed::{BaselineShift, Length, LengthPercentage, Percentage};
    use style::values::generics::box_::BaselineShiftKeyword;

    #[test]
    fn implicit_and_explicit_zero_preserve_the_same_paint_anchor() {
        assert!(geometric_baseline_shift_is_zero(
            BaselineShift::implicit_zero()
        ));
        assert!(geometric_baseline_shift_is_zero(BaselineShift::zero()));
        assert!(geometric_baseline_shift_is_zero(BaselineShift::Length(
            LengthPercentage::new_percent(Percentage(0.0)),
        )));
        for value in [-1.0, 1.0] {
            assert!(!geometric_baseline_shift_is_zero(BaselineShift::Length(
                LengthPercentage::new_length(Length::new(value)),
            )));
        }
        assert!(!geometric_baseline_shift_is_zero(BaselineShift::Keyword(
            BaselineShiftKeyword::Sub,
        )));
    }

    #[test]
    fn materialized_parent_phase_precedes_device_hinting() {
        // The descendant baseline is fractional even after its ancestor's paint anchor snaps.
        assert_eq!(39.25 + f64::from(paint_shift(20.0, 20.0, 2.75, 1.0)), 39.5);
        assert_eq!(39.25 + f64::from(paint_shift(20.0, 20.0, 10.25, 1.0)), 39.0);
    }

    #[test]
    fn negative_origin_uses_unrounded_layout_before_paint_translation() {
        assert_eq!(paint_shift(-0.375, 0.0, 2.75, 1.0), -0.75);
        assert_eq!(paint_shift(20.375, 20.0, 2.75, 1.0), 0.25);
    }

    #[test]
    fn css_anchor_is_snapped_before_scaling() {
        for scale in [1.0, 1.25, 2.0] {
            assert_eq!(paint_shift(20.0, 20.0, -0.5, scale), (0.5 * scale) as f32);
        }
    }

    #[test]
    fn css_negative_half_ties_round_toward_positive_infinity() {
        assert_eq!(paint_shift(-0.5, -0.5, 0.0, 1.0), 0.5);
        assert_eq!(paint_shift(-1.5, -1.5, 0.0, 1.0), 0.5);
    }
}
