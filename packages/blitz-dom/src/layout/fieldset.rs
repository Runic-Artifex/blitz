//! Layout of a `<fieldset>` with a rendered legend
//! (<https://html.spec.whatwg.org/multipage/rendering.html#the-fieldset-and-legend-elements>),
//! as Chromium's `FieldsetLayoutAlgorithm` does it.
//!
//! The rendered legend (`Node::rendered_legend`, the fieldset's first layout
//! child) is taken out of the content the fieldset's `display` lays out (the
//! anonymous fieldset content box): it is shrink-to-fit in the fieldset's
//! content box, placed at the inline start of that box as a block would be
//! (auto margins align it), and centred over the block-start border when the
//! border is thicker than its border box. Its block-start margin is ignored
//! (Chromium). The content starts below the larger of the border and the
//! legend's margin box end, so the content is laid out with that as its
//! block-start border (`Node::fieldset_content_border`); the border itself is
//! painted where the legend's centre is (blitz-paint).
//!
//! A definite block size keeps its border box: the space the legend takes
//! beyond the border comes out of the content box, which does not become
//! negative. When the fieldset is sized by its content, the legend's inline
//! size contributes to its width.

use taffy::{
    AvailableSpace, BoxSizing, CoreStyle as _, Layout, LayoutInput, LayoutOutput,
    LayoutPartialTree as _, Line, MaybeMath as _, MaybeResolve as _, NodeId, Point, Rect,
    RequestedAxis, ResolveOrZero as _, RunMode, Size, SizingMode,
};
use thin_vec::ThinVec;

use super::resolve_calc_value;
use crate::{BaseDocument, dom_node_id, taffy_node_id};

/// A Blitz node id (`NodeId` here is taffy's).
type DomId = crate::NodeId;

pub(super) fn compute_fieldset_layout(
    doc: &mut BaseDocument,
    node_id: NodeId,
    inputs: LayoutInput,
    legend_id: DomId,
) -> LayoutOutput {
    let id = dom_node_id(node_id);
    let legend = taffy_node_id(legend_id);
    let parent_width = inputs.parent_size.width;

    // The fieldset's own box, with its real border.
    let (border, padding, box_sizing, size, min_size, max_size, rtl) = {
        let style = doc.nodes[id].layout_style();
        (
            style
                .border()
                .resolve_or_zero(parent_width, resolve_calc_value),
            style
                .padding()
                .resolve_or_zero(parent_width, resolve_calc_value),
            style.box_sizing(),
            style.size(),
            style.min_size(),
            style.max_size(),
            style.direction() == taffy::Direction::Rtl,
        )
    };
    let bp_inline = border.left + border.right + padding.left + padding.right;
    let bp_block = border.top + border.bottom + padding.top + padding.bottom;
    let border_box = |value: Option<f32>, bp: f32| {
        value.map(|v| match box_sizing {
            BoxSizing::ContentBox => v + bp,
            BoxSizing::BorderBox => v.max(bp),
        })
    };
    let inherent = inputs.sizing_mode == SizingMode::InherentSize;

    // The border-box width: given, or the content's, widened by the legend's
    // contribution when the width is auto.
    let width = match inputs.known_dimensions.width {
        Some(width) => width,
        None => {
            let content = fieldset_content_layout(
                doc,
                node_id,
                LayoutInput {
                    run_mode: RunMode::ComputeSize,
                    axis: RequestedAxis::Horizontal,
                    ..inputs
                },
                legend_id,
                border,
            );
            let mut width = content.size.width;
            if !inherent || size.width.is_auto() {
                let max_width = if inherent {
                    border_box(
                        max_size
                            .width
                            .maybe_resolve(parent_width, resolve_calc_value),
                        bp_inline,
                    )
                } else {
                    None
                };
                let legend_width = legend_inline_contribution(
                    doc,
                    legend,
                    inputs.available_space.width,
                    bp_inline,
                );
                width = width.max((legend_width + bp_inline).maybe_min(max_width));
            }
            if inputs.run_mode == RunMode::ComputeSize && inputs.axis == RequestedAxis::Horizontal {
                return LayoutOutput::from_outer_size(Size {
                    width,
                    height: content.size.height,
                });
            }
            width
        }
    };
    let content_width = (width - bp_inline).max(0.0);

    // The legend: shrink-to-fit in the content box.
    let (margin, position, inset, auto_width, legend_border, legend_padding) = {
        let style = doc.nodes[legend_id].layout_style();
        (
            style
                .margin()
                .map(|m| m.maybe_resolve(Some(content_width), resolve_calc_value)),
            style.position(),
            style.inset(),
            style.size().width.is_auto(),
            style
                .border()
                .resolve_or_zero(Some(content_width), resolve_calc_value),
            style
                .padding()
                .resolve_or_zero(Some(content_width), resolve_calc_value),
        )
    };
    let fixed_margins = margin.left.unwrap_or(0.0) + margin.right.unwrap_or(0.0);
    let available = (content_width - fixed_margins).max(0.0);
    let legend_parent = Size {
        width: Some(content_width),
        height: None,
    };
    let legend_width = auto_width.then(|| {
        let min = measure_legend(doc, legend, legend_parent, AvailableSpace::MinContent);
        let max = measure_legend(doc, legend, legend_parent, AvailableSpace::MaxContent);
        available.min(max).max(min)
    });
    let legend_output = doc.compute_child_layout(
        legend,
        LayoutInput {
            run_mode: inputs.run_mode,
            sizing_mode: SizingMode::InherentSize,
            axis: RequestedAxis::Both,
            known_dimensions: Size {
                width: legend_width,
                height: None,
            },
            known_dimensions_are_definite: Size {
                width: true,
                height: true,
            },
            parent_size: legend_parent,
            available_space: Size {
                width: AvailableSpace::Definite(available),
                height: AvailableSpace::MaxContent,
            },
            vertical_margins_are_collapsible: Line::FALSE,
        },
    );
    let legend_size = legend_output.size;

    // Inline position: as a block in the content box, auto margins aligning it.
    let free = (content_width - legend_size.width - fixed_margins).max(0.0);
    let (margin_left, margin_right) = match (margin.left, margin.right) {
        (None, None) => (free / 2.0, free / 2.0),
        (None, Some(right)) => (free, right),
        (Some(left), None) => (left, free),
        (Some(left), Some(right)) => (left, right),
    };
    let x = if rtl {
        width - border.right - padding.right - margin_right - legend_size.width
    } else {
        border.left + padding.left + margin_left
    };
    // Block position: centred over a thicker border, else at the top (the
    // block-start margin is ignored).
    let space_left = border.top - legend_size.height;
    let y = if space_left > 0.0 {
        space_left / 2.0
    } else {
        0.0
    };
    let margin_bottom = margin.bottom.unwrap_or(0.0);
    let content_top = border.top.max(y + legend_size.height + margin_bottom);

    // The block size: a definite one keeps its border box, the content box
    // giving up the space the legend spills past the border.
    let min_height = content_top + padding.top + padding.bottom + border.bottom;
    let mut height = inputs.known_dimensions.height;
    let mut height_is_definite = inputs.known_dimensions_are_definite.height;
    if height.is_none() && inherent {
        let parent_height = inputs.parent_size.height;
        if let Some(h) = border_box(
            size.height.maybe_resolve(parent_height, resolve_calc_value),
            bp_block,
        ) {
            let min = border_box(
                min_size
                    .height
                    .maybe_resolve(parent_height, resolve_calc_value),
                bp_block,
            );
            let max = border_box(
                max_size
                    .height
                    .maybe_resolve(parent_height, resolve_calc_value),
                bp_block,
            );
            height = Some(h.maybe_min(max).maybe_max(min));
            height_is_definite = true;
        }
    }
    let height = height.map(|h| h.max(min_height));

    let mut output = fieldset_content_layout(
        doc,
        node_id,
        LayoutInput {
            known_dimensions: Size {
                width: Some(width),
                height,
            },
            known_dimensions_are_definite: Size {
                width: inputs.known_dimensions_are_definite.width
                    || inputs.known_dimensions.width.is_none(),
                height: height_is_definite || inputs.known_dimensions.height.is_none(),
            },
            ..inputs
        },
        legend_id,
        Rect {
            top: content_top,
            ..border
        },
    );

    if inputs.run_mode == RunMode::PerformLayout {
        let mut location = Point { x, y };
        // A relatively positioned legend moves (the content stays).
        if position == taffy::Position::Relative {
            let left = inset
                .left
                .maybe_resolve(Some(content_width), resolve_calc_value);
            let right = inset
                .right
                .maybe_resolve(Some(content_width), resolve_calc_value);
            let top = inset.top.maybe_resolve(None, resolve_calc_value);
            let bottom = inset.bottom.maybe_resolve(None, resolve_calc_value);
            location.x += left.or(right.map(|r| -r)).unwrap_or(0.0);
            location.y += top.or(bottom.map(|b| -b)).unwrap_or(0.0);
        }
        doc.set_unrounded_layout(
            legend,
            &Layout {
                order: 0,
                location,
                size: legend_size,
                scrollable_overflow_rect: legend_output.scrollable_overflow_rect,
                scrollbar_size: Size::ZERO,
                border: legend_border,
                padding: legend_padding,
                margin: Rect {
                    left: margin_left,
                    right: margin_right,
                    top: margin.top.unwrap_or(0.0),
                    bottom: margin_bottom,
                },
            },
        );
        let mut candidates = legend_output.oof_candidates;
        if !candidates.is_empty() {
            candidates.translate(location);
            candidates.append(&mut output.oof_candidates);
            output.oof_candidates = candidates;
        }

        // The content's overflow is measured from the padding box it was laid
        // out with; move it to the fieldset's, and add the legend.
        let shift = content_top - border.top;
        let overflow = &mut output.scrollable_overflow_rect;
        overflow.top = (overflow.top + shift).min(0.0);
        overflow.bottom += shift;
        if !rtl {
            overflow.left = overflow.left.min(location.x - border.left);
            overflow.right = overflow
                .right
                .max(location.x - border.left + legend_size.width);
            overflow.top = overflow.top.min(location.y - border.top);
            overflow.bottom = overflow
                .bottom
                .max(location.y - border.top + legend_size.height);
        }
    }

    output
}

/// Lay out the fieldset's content, without its rendered legend, with `border`
/// (the block-start border moved below the legend).
fn fieldset_content_layout(
    doc: &mut BaseDocument,
    node_id: NodeId,
    inputs: LayoutInput,
    legend_id: DomId,
    border: Rect<f32>,
) -> LayoutOutput {
    let id = dom_node_id(node_id);
    let children = doc.nodes[id].layout_children.borrow_mut().take();
    let content: ThinVec<DomId> = children
        .iter()
        .flatten()
        .copied()
        .filter(|&child| child != legend_id)
        .collect();
    *doc.nodes[id].layout_children.borrow_mut() = Some(content);
    doc.nodes[id].fieldset_content_border.set(Some(border));
    let output = doc.compute_display_layout(node_id, inputs, None);
    doc.nodes[id].fieldset_content_border.set(None);
    *doc.nodes[id].layout_children.borrow_mut() = children;
    output
}

/// The legend's intrinsic border-box width.
fn measure_legend(
    doc: &mut BaseDocument,
    legend: NodeId,
    parent_size: Size<Option<f32>>,
    available: AvailableSpace,
) -> f32 {
    doc.compute_child_layout(
        legend,
        LayoutInput {
            known_dimensions: Size::NONE,
            known_dimensions_are_definite: Size {
                width: true,
                height: true,
            },
            parent_size,
            available_space: Size {
                width: available,
                height: AvailableSpace::MaxContent,
            },
            sizing_mode: SizingMode::InherentSize,
            axis: RequestedAxis::Horizontal,
            run_mode: RunMode::ComputeSize,
            vertical_margins_are_collapsible: Line::FALSE,
        },
    )
    .size
    .width
}

/// The legend's margin-box width in a fieldset sized by its content
/// (`available`: the fieldset's available inline space).
fn legend_inline_contribution(
    doc: &mut BaseDocument,
    legend: NodeId,
    available: AvailableSpace,
    bp_inline: f32,
) -> f32 {
    let margins = {
        let style = doc.nodes[dom_node_id(legend)].layout_style();
        let margin = style.margin().resolve_or_zero(None, resolve_calc_value);
        margin.left + margin.right
    };
    let measure = |doc: &mut BaseDocument, space| measure_legend(doc, legend, Size::NONE, space);
    let width = match available {
        AvailableSpace::MinContent => measure(doc, AvailableSpace::MinContent),
        AvailableSpace::MaxContent => measure(doc, AvailableSpace::MaxContent),
        AvailableSpace::Definite(space) => {
            let min = measure(doc, AvailableSpace::MinContent);
            let max = measure(doc, AvailableSpace::MaxContent);
            (space - bp_inline - margins).min(max).max(min)
        }
    };
    width + margins
}

#[cfg(test)]
mod tests {
    //! Geometry as measured in Chromium 153 (boxes without text, so no fonts
    //! are needed; the UA sheet's legend padding-inline is 2px).

    use blitz_traits::shell::{ColorScheme, Viewport};

    use crate::{Attribute, BaseDocument, DocumentConfig, NodeId, qual_name};

    /// `(tag, style, children)`
    struct El(&'static str, &'static str, Vec<El>);

    fn el(tag: &'static str, style: &'static str, children: Vec<El>) -> El {
        El(tag, style, children)
    }

    /// A legend with a `width` x `height` block in it.
    fn legend(style: &'static str, width_height: &'static str) -> El {
        el("legend", style, vec![el("div", width_height, vec![])])
    }

    /// Resolve `<html><body style="margin:0">{fieldset}</body></html>`;
    /// the ids of the elements in document order (the fieldset first).
    fn resolve(fieldset: El) -> (BaseDocument, Vec<NodeId>) {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root = doc.root_node().id;
        let mut ids = Vec::new();
        let mut mutator = doc.mutate();
        fn build(
            mutator: &mut crate::DocumentMutator<'_>,
            El(tag, style, children): El,
            ids: &mut Vec<NodeId>,
        ) -> NodeId {
            let name = match tag {
                "fieldset" => qual_name!("fieldset", html),
                "legend" => qual_name!("legend", html),
                _ => qual_name!("div", html),
            };
            let attrs = vec![Attribute {
                name: qual_name!("style"),
                value: style.to_string(),
            }];
            let id = mutator.create_element(name, attrs);
            ids.push(id);
            let children: Vec<NodeId> = children
                .into_iter()
                .map(|child| build(mutator, child, ids))
                .collect();
            mutator.append_children(id, &children);
            id
        }
        let html = mutator.create_element(qual_name!("html", html), vec![]);
        let body = mutator.create_element(
            qual_name!("body", html),
            vec![Attribute {
                name: qual_name!("style"),
                value: "margin:0".to_string(),
            }],
        );
        let fieldset = build(&mut mutator, fieldset, &mut ids);
        mutator.append_children(body, &[fieldset]);
        mutator.append_children(html, &[body]);
        mutator.append_children(root, &[html]);
        drop(mutator);
        doc.resolve(0.0);
        (doc, ids)
    }

    /// `[x, y, width, height]` of the border box, relative to the parent.
    fn rect(doc: &BaseDocument, id: NodeId) -> [f32; 4] {
        let layout = doc.nodes[id].final_layout();
        [
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height,
        ]
    }

    const UA: &str = "margin:0; width:200px; padding:4px 10px 6px";

    #[test]
    fn ua_legend_straddles_the_border() {
        // [fieldset, legend, legend's block, content]
        let (doc, ids) = resolve(el(
            "fieldset",
            UA,
            vec![
                legend("", "width:50px; height:20px"),
                el("div", "height:10px", vec![]),
            ],
        ));
        assert_eq!(doc.nodes[ids[0]].rendered_legend(), Some(ids[1]));
        // Inline start of the content box, at the top; shrink-to-fit.
        assert_eq!(rect(&doc, ids[1]), [12.0, 0.0, 54.0, 20.0]);
        // The content starts below the legend (not the 2px border).
        assert_eq!(rect(&doc, ids[3]), [12.0, 24.0, 200.0, 10.0]);
        assert_eq!(rect(&doc, ids[0]), [0.0, 0.0, 224.0, 42.0]);
    }

    #[test]
    fn legend_leaves_the_flex_gap() {
        // shadcn's FieldSet: a reset flex column with a gap, the legend with
        // a block-end margin.
        let (doc, ids) = resolve(el(
            "fieldset",
            "margin:0; padding:0; border:0; width:300px; display:flex; flex-direction:column; gap:24px",
            vec![
                legend("padding:0; margin-bottom:12px", "width:50px; height:24px"),
                el("div", "height:10px", vec![]),
                el("div", "height:10px", vec![]),
            ],
        ));
        assert_eq!(rect(&doc, ids[1]), [0.0, 0.0, 50.0, 24.0]);
        assert_eq!(rect(&doc, ids[3]), [0.0, 36.0, 300.0, 10.0]);
        assert_eq!(rect(&doc, ids[4]), [0.0, 70.0, 300.0, 10.0]);
        assert_eq!(rect(&doc, ids[0]), [0.0, 0.0, 300.0, 80.0]);
    }

    #[test]
    fn thick_border_centres_the_legend() {
        let (doc, ids) = resolve(el(
            "fieldset",
            "margin:0; width:200px; padding:0; border:40px solid",
            vec![
                legend("", "width:50px; height:20px"),
                el("div", "height:10px", vec![]),
            ],
        ));
        assert_eq!(rect(&doc, ids[1]), [40.0, 10.0, 54.0, 20.0]);
        assert_eq!(rect(&doc, ids[3]), [40.0, 40.0, 200.0, 10.0]);
    }

    #[test]
    fn block_start_margin_is_ignored_and_auto_margins_align() {
        let (doc, ids) = resolve(el(
            "fieldset",
            UA,
            vec![
                legend("margin:30px 0 4px auto", "width:50px; height:20px"),
                el("div", "height:10px", vec![]),
            ],
        ));
        assert_eq!(rect(&doc, ids[1]), [158.0, 0.0, 54.0, 20.0]);
        assert_eq!(rect(&doc, ids[3]), [12.0, 28.0, 200.0, 10.0]);
    }

    #[test]
    fn first_legend_anywhere_is_rendered_and_a_second_stays_in_the_content() {
        // [fieldset, before, legend, block, second legend, block]
        let (doc, ids) = resolve(el(
            "fieldset",
            UA,
            vec![
                el("div", "height:10px", vec![]),
                legend("", "width:50px; height:20px"),
                legend("", "width:30px; height:8px"),
            ],
        ));
        assert_eq!(doc.nodes[ids[0]].rendered_legend(), Some(ids[2]));
        assert_eq!(rect(&doc, ids[2]), [12.0, 0.0, 54.0, 20.0]);
        assert_eq!(rect(&doc, ids[1]), [12.0, 24.0, 200.0, 10.0]);
        // A full-width block after the first content.
        assert_eq!(rect(&doc, ids[4]), [12.0, 34.0, 200.0, 8.0]);
    }

    #[test]
    fn floated_legend_is_not_rendered() {
        let (doc, ids) = resolve(el(
            "fieldset",
            UA,
            vec![legend("float:left", "width:50px; height:20px")],
        ));
        assert_eq!(doc.nodes[ids[0]].rendered_legend(), None);
    }

    #[test]
    fn definite_height_keeps_its_border_box() {
        let (doc, ids) = resolve(el(
            "fieldset",
            "margin:0; width:200px; padding:4px 10px 6px; height:60px",
            vec![
                legend("", "width:50px; height:20px"),
                el("div", "height:10px", vec![]),
            ],
        ));
        // 60 + 4 + 6 + 2 + 2, the legend spill out of the content box.
        assert_eq!(rect(&doc, ids[0]), [0.0, 0.0, 224.0, 74.0]);
        assert_eq!(rect(&doc, ids[3]), [12.0, 24.0, 200.0, 10.0]);
    }

    #[test]
    fn shrink_to_fit_fieldset_is_as_wide_as_its_legend() {
        let (doc, ids) = resolve(el(
            "fieldset",
            "margin:0; padding:4px 10px 6px; display:inline-block",
            vec![
                legend("", "width:100px; height:20px"),
                el("div", "width:10px; height:10px", vec![]),
            ],
        ));
        // 100 + 2 * 2 legend padding + 2 * 10 padding + 2 * 2 border.
        assert_eq!(rect(&doc, ids[0])[2], 128.0);
        assert_eq!(rect(&doc, ids[1]), [12.0, 0.0, 104.0, 20.0]);
    }

    #[test]
    fn grid_content_excludes_the_legend() {
        let (doc, ids) = resolve(el(
            "fieldset",
            "margin:0; width:200px; padding:4px 10px 6px; display:grid; grid-template-columns:1fr 1fr; gap:4px",
            vec![
                legend("", "width:50px; height:20px"),
                el("div", "height:10px", vec![]),
                el("div", "height:10px", vec![]),
            ],
        ));
        assert_eq!(rect(&doc, ids[1]), [12.0, 0.0, 54.0, 20.0]);
        assert_eq!(rect(&doc, ids[3]), [12.0, 24.0, 98.0, 10.0]);
        assert_eq!(rect(&doc, ids[4]), [114.0, 24.0, 98.0, 10.0]);
    }
}
