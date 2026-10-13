//! `-webkit-line-clamp`, laid out as Chromium 153 does with its `CSSLineClamp`
//! runtime flag off, the legacy behaviour (Blink's `BlockLineClampData` in
//! `core/layout/block_layout_algorithm.cc`, `GetLineClampState` in
//! `core/layout/inline/inline_layout_algorithm.cc` and `LineTruncator` in
//! `core/layout/inline/line_truncator.cc`).
//!
//! A box whose specified `display` is `-webkit-box` or `-webkit-inline-box`,
//! with `-webkit-box-orient: vertical` and a `-webkit-line-clamp` count, is laid
//! out as a `flow-root` or an `inline-block` (Stylo's style adjuster) that
//! clamps after that many lines. Lines are counted through its in-flow blocks
//! that do not establish formatting contexts. If anything follows the clamp
//! line, more lines or any other box:
//!
//! - the clamp line ends in an ellipsis, and
//! - every block of the formatting context in which the clamp point has been
//!   passed (the clamp root, the blocks around the clamp line, and the blocks
//!   after it) ends after its first line or child past that point, plus its
//!   block-end border and padding, unless its block size is fixed.
//!
//! Later boxes are laid out after the shortened ones and stay visible unless
//! clipped. If nothing follows the clamp line, nothing is clamped.

use blitz_traits::node_id::NodeId;
use parley::PositionedLayoutItem;
use parley::layout::BreakReason;
use style::properties::ComputedValues;
use style::values::specified::box_::DisplayInside;
use taffy::{
    BlockItemStyle as _, BoxSizing, CoreStyle as _, Layout, LayoutInput, LayoutOutput,
    MaybeMath as _, MaybeResolve as _, RequestedAxis, ResolveOrZero as _, RunMode, SizingMode,
};

use super::resolve_calc_value;
use crate::BaseDocument;
use crate::node::{LineClampEllipsis, TextLayout};

pub(crate) const LINE_CLAMP_RTL: &str =
    "-webkit-line-clamp on a line of right-to-left or bidirectional text";
pub(crate) const LINE_CLAMP_ALIGN: &str = "-webkit-line-clamp on a line that is not start-aligned (text-align center, right, end or justify)";
pub(crate) const LINE_CLAMP_ATOMIC: &str =
    "an atomic inline box after a -webkit-line-clamp ellipsis (painted rather than hidden)";
pub(crate) const LINE_CLAMP_FONT: &str = "a -webkit-line-clamp ellipsis in a block without text of its own (drawn in the font of its first text)";
pub(crate) const LINE_CLAMP_MARGIN: &str =
    "a block shortened by -webkit-line-clamp whose block-end margin collapses with its content's";
pub(crate) const LINE_CLAMP_PERCENT: &str =
    "a percentage block size on a block shortened by -webkit-line-clamp (kept)";
pub(crate) const LINE_CLAMP_OUT_OF_FLOW: &str =
    "an out-of-flow box after a -webkit-line-clamp clamp point or in a block it shortens";
pub(crate) const LEGACY_WEBKIT_BOX: &str = "display: -webkit-box or -webkit-inline-box without -webkit-box-orient: vertical and a -webkit-line-clamp count (a legacy flexible box, laid out as a block)";
pub(crate) const LINE_CLAMP_BUTTON: &str =
    "-webkit-line-clamp on a button (laid out as the button's flex column, unclamped)";

/// The number of lines after which `style`'s box clamps, if it is a
/// `-webkit-line-clamp` root: its specified `display` is `-webkit-box` or
/// `-webkit-inline-box` and Stylo turned it into `flow-root` or `inline-block`
/// for `-webkit-box-orient: vertical` and a `-webkit-line-clamp` count.
pub(crate) fn clamp_lines(style: &ComputedValues) -> Option<usize> {
    let box_style = style.get_box();
    if box_style.original_display.inside() != DisplayInside::WebkitBox
        || box_style.display.inside() != DisplayInside::FlowRoot
    {
        return None;
    }
    let line_clamp = box_style.clone_line_clamp();
    let lines = line_clamp.max_lines.lines_value()?;
    usize::try_from(lines.0).ok().filter(|&lines| lines > 0)
}

/// Report the box of `node_id` if it is a legacy box Blitz does not lay out
/// as Chromium does: a `-webkit-box` Stylo did not turn into a line clamp
/// root (Chromium lays it out as a legacy flexible box), or a line clamp root
/// that is a button.
pub(crate) fn report_legacy_box(doc: &mut BaseDocument, node_id: NodeId) {
    let report = {
        let node = &doc.nodes[node_id];
        let Some(style) = node.primary_styles() else {
            return;
        };
        if style.get_box().display.inside() == DisplayInside::WebkitBox {
            LEGACY_WEBKIT_BOX
        } else if node.is_flow_button() && clamp_lines(&style).is_some() {
            LINE_CLAMP_BUTTON
        } else {
            return;
        }
    };
    doc.report_unsupported_layout(report);
}

/// The number of line boxes of an inline formatting context: none without
/// inline content, and without the empty line Parley adds after a forced
/// break that ends the content (as `compute_inline_layout_inner` measures).
pub(crate) fn formatted_line_count(text: &TextLayout) -> usize {
    if text.text.is_empty() && text.layout.inline_boxes().len() == 0 {
        return 0;
    }
    let count = text.layout.len();
    if count >= 2
        && let (Some(prev_line), Some(last_line)) =
            (text.layout.get(count - 2), text.layout.get(count - 1))
        && prev_line.break_reason() == BreakReason::Explicit
        && last_line.text_range().is_empty()
        && last_line.items().next().is_none()
    {
        return count - 1;
    }
    count
}

impl BaseDocument {
    /// The `-webkit-line-clamp` ellipsis of the inline formatting context of
    /// `node_id`, if its clamp root (this block or a block ancestor) still
    /// clamps by lines.
    pub fn line_clamp_ellipsis(&self, node_id: NodeId) -> Option<&LineClampEllipsis> {
        let node = self.get_node(node_id)?;
        let clamp = node
            .element_data()?
            .inline_layout_data
            .as_ref()?
            .line_clamp
            .as_deref()?;
        let mut ancestor = Some(node.id);
        while let Some(id) = ancestor {
            if id == clamp.root {
                break;
            }
            ancestor = self.nodes[id].layout_parent.get();
        }
        ancestor?;
        let root_style = self.nodes[clamp.root].primary_styles()?;
        clamp_lines(&root_style)?;
        Some(clamp)
    }

    /// The ellipsis that Chromium's `LineTruncator` places on line `line` of
    /// the inline formatting context `ifc` (whose layout is `text`), with the
    /// inline end of its content box at `edge` (layout units).
    pub(crate) fn plan_line_clamp_ellipsis(
        &self,
        ifc: NodeId,
        text: &TextLayout,
        root: NodeId,
        line_index: usize,
        edge: f32,
        reports: &mut Vec<&'static str>,
    ) -> Option<LineClampEllipsis> {
        let line = text.layout.get(line_index)?;
        let style = self.nodes[ifc].primary_styles()?;
        let runs = glyph_runs;

        if !style.writing_mode.is_bidi_ltr() || runs(line).iter().any(|(_, run)| run.run().is_rtl())
        {
            reports.push(LINE_CLAMP_RTL);
        }
        let justify = matches!(
            style.clone_text_align(),
            style::values::specified::TextAlignKeyword::Justify
        );
        if justify || line.metrics().offset > 0.01 {
            reports.push(LINE_CLAMP_ALIGN);
        }

        // The ellipsis is shaped in the primary font of the block's style:
        // that of the block's own text, preferring this line's.
        let own_run = |line_number: usize| {
            let line = text.layout.get(line_number)?;
            runs(line)
                .into_iter()
                .find(|(_, run)| run.style().brush.id == ifc)
                .map(|(index, run)| (line_number, index, usize::from(run.style_index())))
        };
        let font_run = own_run(line_index)
            .or_else(|| (0..text.layout.len()).find_map(own_run))
            .or_else(|| {
                let (index, run) = runs(line).into_iter().next()?;
                reports.push(LINE_CLAMP_FONT);
                Some((line_index, index, usize::from(run.style_index())))
            })?;
        let (glyphs, advance) = {
            let font_line = text.layout.get(font_run.0)?;
            let Some(PositionedLayoutItem::GlyphRun(run)) = font_line.items().nth(font_run.1)
            else {
                return None;
            };
            let run = run.run();
            let coords: Vec<i16> = run
                .normalized_coords()
                .iter()
                .map(|c| c.to_bits())
                .collect();
            ellipsis_glyphs(&run.font().font, run.font_size(), &coords)?
        };

        // The end of the line's content: the last glyph run or atomic inline
        // box, without trailing collapsible whitespace or inline box edges.
        let metrics = line.metrics();
        let item_count = line.items().count();
        let mut content_end = metrics.offset;
        for (index, item) in line.items().enumerate() {
            let end = match item {
                PositionedLayoutItem::GlyphRun(run) => {
                    let (_, right_edge) = run.inline_edge_insets();
                    let hanging = if index + 1 == item_count {
                        metrics.hanging_advance
                    } else {
                        0.0
                    };
                    run.offset() + run.advance() - right_edge - hanging
                }
                PositionedLayoutItem::InlineBox(ibox) => ibox.x + ibox.width,
            };
            content_end = content_end.max(end);
        }

        let (x, cutoff) = if content_end + advance <= edge + 0.01 {
            // Everything fits: the ellipsis follows the line's content.
            (content_end, None)
        } else {
            // The glyphs that fit before the ellipsis (the character offset
            // to fit of the truncated text), keeping at least the first
            // character of the line.
            let cutoff = edge - advance;
            let mut visible_end = None::<f32>;
            let mut first_glyph_end = None::<f32>;
            for (_, run) in runs(line) {
                for glyph in run.positioned_glyphs() {
                    let end = glyph.x + glyph.advance;
                    first_glyph_end.get_or_insert(end);
                    if end <= cutoff + 0.01 {
                        visible_end = Some(visible_end.map_or(end, |v| v.max(end)));
                    }
                }
            }
            let visible_end = visible_end.or(first_glyph_end).unwrap_or(metrics.offset);
            // Atomic inline boxes that do not fit are hidden by Chromium.
            if line.items().any(|item| {
                matches!(item, PositionedLayoutItem::InlineBox(ibox)
                    if ibox.x + ibox.width > visible_end.max(cutoff) + 0.01)
            }) {
                reports.push(LINE_CLAMP_ATOMIC);
            }
            (visible_end, Some(visible_end.max(cutoff)))
        };

        Some(LineClampEllipsis {
            root,
            line: line_index,
            cutoff,
            x,
            style_index: font_run.2,
            font_run: (font_run.0, font_run.1),
            glyphs,
            advance,
        })
    }

    /// The layout of a `-webkit-line-clamp` root whose content is not a single
    /// inline formatting context (that case is clamped by the inline layout).
    pub(crate) fn compute_line_clamp_root_layout(
        &mut self,
        node_id: NodeId,
        inputs: LayoutInput,
        lines: usize,
    ) -> LayoutOutput {
        let taffy_id = crate::taffy_node_id(node_id);
        let clamps = match inputs.run_mode {
            RunMode::PerformLayout => true,
            RunMode::ComputeSize => inputs.axis != RequestedAxis::Horizontal,
            RunMode::PerformHiddenLayout => false,
        };
        if !clamps {
            return taffy::compute_block_layout(self, taffy_id, inputs, None);
        }
        // The clamp is found in the laid-out lines, so the block size is
        // measured with a full layout of the content.
        let mut output = taffy::compute_block_layout(
            self,
            taffy_id,
            LayoutInput {
                run_mode: RunMode::PerformLayout,
                ..inputs
            },
            None,
        );

        // The root's own padding and border, as Taffy resolves them.
        let (root_padding, root_border) = {
            let style = self.nodes[node_id].layout_style();
            (
                style
                    .padding()
                    .resolve_or_zero(inputs.parent_size.width, resolve_calc_value),
                style
                    .border()
                    .resolve_or_zero(inputs.parent_size.width, resolve_calc_value),
            )
        };
        let root_pb = root_padding + root_border;
        let mut walk = Walk {
            doc: self,
            lines_until_clamp: lines as isize,
            clamp: None,
            heights: Vec::new(),
            shifts: Vec::new(),
            parents: Vec::new(),
            ifcs: Vec::new(),
            reports: Vec::new(),
        };
        let saved = walk.container(node_id, 0.0, Some(root_pb.top));
        let Walk {
            lines_until_clamp,
            clamp,
            heights,
            shifts,
            parents,
            ifcs,
            mut reports,
            ..
        } = walk;

        for ifc in &ifcs {
            if let Some(text) = self.nodes[*ifc]
                .data
                .downcast_element_mut()
                .and_then(|element| element.inline_layout_data.as_mut())
            {
                text.line_clamp = None;
            }
        }

        // Clamped only if content follows the clamp line; otherwise Chromium
        // lays the box out again ignoring the clamp.
        let (Some(saved), Some((ifc, line_index, baseline))) = (saved, clamp) else {
            return output;
        };
        if lines_until_clamp >= 0 {
            return output;
        }

        output.size.height =
            self.line_clamp_root_height(node_id, inputs, saved + root_pb.bottom, &output);
        output.baselines.last = Some(baseline);

        for (id, height) in heights {
            self.nodes[id].unrounded_layout_mut().size.height = height;
        }
        for (id, shift) in shifts {
            self.nodes[id].unrounded_layout_mut().location.y -= shift;
        }
        // The blocks whose children were changed lay them out again next
        // time, and their scrollable overflow follows the moved boxes
        // (innermost first).
        let depth = |doc: &BaseDocument, mut id: NodeId| {
            let mut depth = 0;
            while id != node_id {
                let Some(parent) = doc.nodes[id].layout_parent.get() else {
                    break;
                };
                id = parent;
                depth += 1;
            }
            depth
        };
        let mut parents = parents;
        parents.sort_unstable();
        parents.dedup();
        parents.sort_by_key(|&id| std::cmp::Reverse(depth(self, id)));
        for id in parents {
            if id == node_id {
                continue;
            }
            self.nodes[id].layout_data_mut().cache.clear();
            let border_top = self.nodes[id].unrounded_layout().border.top;
            let padding_bottom = self.nodes[id].unrounded_layout().padding.bottom;
            let bottom = self.inflow_overflow_bottom(id, border_top, padding_bottom);
            self.nodes[id]
                .unrounded_layout_mut()
                .scrollable_overflow_rect
                .bottom = bottom;
        }
        output.scrollable_overflow_rect.bottom =
            self.inflow_overflow_bottom(node_id, root_border.top, root_padding.bottom);

        let scale = self.viewport.scale();
        let mut text = self.nodes[ifc]
            .data
            .downcast_element_mut()
            .and_then(|element| element.take_inline_layout());
        if let Some(text) = text.as_mut() {
            let edge = self.nodes[ifc].unrounded_layout().content_box_width() * scale;
            text.line_clamp = self
                .plan_line_clamp_ellipsis(ifc, text, node_id, line_index, edge, &mut reports)
                .map(Box::new);
        }
        if let Some(element) = self.nodes[ifc].data.downcast_element_mut() {
            element.inline_layout_data = text;
        }
        for report in reports {
            self.report_unsupported_layout(report);
        }
        output
    }

    /// The block-end edge of the scrollable overflow of the in-flow children
    /// of the block container `node_id`, as Taffy's block layout computes
    /// it (relative to the padding box).
    fn inflow_overflow_bottom(&self, node_id: NodeId, border_top: f32, padding_bottom: f32) -> f32 {
        let node = &self.nodes[node_id];
        let is_scroll_container = {
            let overflow = node.layout_style().overflow();
            overflow.x.is_scroll_container() || overflow.y.is_scroll_container()
        };
        let mut bottom = 0.0_f32;
        for &child_id in node.layout_children.borrow().iter().flatten() {
            let child = &self.nodes[child_id];
            if child.primary_styles().is_none() || child.is_out_of_flow() {
                continue;
            }
            let style = child.layout_style();
            if style.box_generation_mode() == taffy::BoxGenerationMode::None {
                continue;
            }
            let overflow = style.overflow();
            let propagates = !overflow.x.is_scroll_container()
                && !overflow.y.is_scroll_container()
                && !style.contain().contains_scrollable_overflow()
                && overflow.y == taffy::Overflow::Visible;
            let layout = child.unrounded_layout();
            let height = if propagates {
                layout
                    .size
                    .height
                    .max(layout.scrollable_overflow_rect.bottom)
            } else {
                layout.size.height
            };
            if height <= 0.0 || layout.size.width <= 0.0 {
                continue;
            }
            bottom = bottom.max(layout.location.y - border_top + height);
        }
        if is_scroll_container {
            bottom += padding_bottom;
        }
        bottom
    }

    /// The border-box block size of a clamp root that ends at `intrinsic`:
    /// its fixed or min/max-clamped size, as `compute_inline_layout` sizes
    /// boxes.
    fn line_clamp_root_height(
        &self,
        node_id: NodeId,
        inputs: LayoutInput,
        intrinsic: f32,
        output: &LayoutOutput,
    ) -> f32 {
        if let Some(height) = inputs.known_dimensions.height {
            return height;
        }
        let style = self.nodes[node_id].layout_style();
        let parent_size = inputs.parent_size;
        let pb = style
            .padding()
            .resolve_or_zero(parent_size.width, resolve_calc_value)
            + style
                .border()
                .resolve_or_zero(parent_size.width, resolve_calc_value);
        let pb_sum = pb.vertical_axis_sum();
        if inputs.sizing_mode == SizingMode::ContentSize {
            return intrinsic.max(pb_sum);
        }
        if style.aspect_ratio().is_some() {
            return output.size.height;
        }
        let adjustment = if style.box_sizing() == BoxSizing::ContentBox {
            pb_sum
        } else {
            0.0
        };
        let min = style
            .min_size()
            .height
            .maybe_resolve(parent_size.height, resolve_calc_value)
            .maybe_add(adjustment);
        let max = style
            .max_size()
            .height
            .maybe_resolve(parent_size.height, resolve_calc_value)
            .maybe_add(adjustment);
        style
            .size()
            .height
            .maybe_resolve(parent_size.height, resolve_calc_value)
            .maybe_add(adjustment)
            .unwrap_or(intrinsic)
            .maybe_clamp(min, max)
            .max(pb_sum)
    }
}

/// The glyph runs of `line` with their item indices.
fn glyph_runs<'a>(
    line: parley::Line<'a, crate::node::TextBrush>,
) -> Vec<(usize, parley::GlyphRun<'a, crate::node::TextBrush>)> {
    line.items()
        .enumerate()
        .filter_map(|(index, item)| match item {
            PositionedLayoutItem::GlyphRun(run) => Some((index, run)),
            _ => None,
        })
        .collect()
}

/// The ellipsis Chromium draws for a line clamp in `font`: U+2026, or three
/// full stops if the font has no glyph for it, with the glyphs' offsets and
/// the total advance (layout units).
fn ellipsis_glyphs(
    font: &parley::FontData,
    font_size: f32,
    coords: &[i16],
) -> Option<(Vec<(u32, f32)>, f32)> {
    use skrifa::MetadataProvider as _;
    let font_ref = skrifa::FontRef::from_index(font.data.as_ref(), font.index).ok()?;
    let coords: Vec<skrifa::instance::NormalizedCoord> = coords
        .iter()
        .map(|&c| skrifa::instance::NormalizedCoord::from_bits(c))
        .collect();
    let metrics = font_ref.glyph_metrics(
        skrifa::instance::Size::new(font_size),
        skrifa::instance::LocationRef::new(&coords),
    );
    let charmap = font_ref.charmap();
    let (glyph, repeat) = match charmap.map('\u{2026}') {
        Some(glyph) => (glyph, 1),
        None => (charmap.map('.')?, 3),
    };
    let glyph_advance = metrics.advance_width(glyph)?;
    let glyphs = (0..repeat)
        .map(|i| (glyph.to_u32(), glyph_advance * i as f32))
        .collect();
    Some((glyphs, glyph_advance * repeat as f32))
}

/// The line count through a clamp root's block formatting context, and what
/// clamping changes, collected without changing any layout.
struct Walk<'a> {
    doc: &'a BaseDocument,
    /// Chromium's `lines_until_clamp`: 1 at the clamp line, then 0 and below.
    lines_until_clamp: isize,
    /// The clamp line: its inline formatting context, index and baseline in
    /// the clamp root's border box.
    clamp: Option<(NodeId, usize, f32)>,
    /// Shortened blocks and their border-box block sizes.
    heights: Vec<(NodeId, f32)>,
    /// Boxes moved up after shortened siblings.
    shifts: Vec<(NodeId, f32)>,
    /// The blocks whose children were shortened or moved.
    parents: Vec<NodeId>,
    /// The inline formatting contexts walked.
    ifcs: Vec<NodeId>,
    reports: Vec<&'static str>,
}

impl Walk<'_> {
    /// Count the lines of the block container `node_id`, whose border box
    /// starts at `top` in the clamp root. Returns its block offset after the
    /// first line or child past the clamp point (Chromium's
    /// `previous_inflow_position_when_clamped`), if any.
    /// `root_pb_top` is the block-start padding and border of the clamp
    /// root, which its parent has not placed yet.
    fn container(&mut self, node_id: NodeId, top: f32, root_pb_top: Option<f32>) -> Option<f32> {
        let doc = self.doc;
        let node = &doc.nodes[node_id];
        let layout = node.unrounded_layout();
        let pb_top = root_pb_top.unwrap_or(layout.padding.top + layout.border.top);
        if node.flags.is_inline_root() {
            return self
                .lines(node_id, top + pb_top)
                .map(|bottom| pb_top + bottom);
        }

        let children: Vec<NodeId> = node
            .layout_children
            .borrow()
            .iter()
            .flatten()
            .copied()
            .collect();
        let mut saved = None;
        let mut shift = 0.0;
        for child_id in children {
            let child = &doc.nodes[child_id];
            let Some(style) = child.primary_styles() else {
                continue;
            };
            let box_style = style.get_box();
            if box_style.display.is_none() {
                continue;
            }
            if child.is_out_of_flow() || box_style.position.is_absolutely_positioned() {
                if self.lines_until_clamp <= 0 {
                    self.reports.push(LINE_CLAMP_OUT_OF_FLOW);
                }
                continue;
            }
            if box_style.float != style::computed_values::float::T::None {
                continue;
            }
            drop(style);

            let child_layout: Layout = *child.unrounded_layout();
            if shift != 0.0 {
                self.shifts.push((child_id, shift));
                self.parents.push(node_id);
            }
            let child_top = child_layout.location.y - shift;
            let before = self.lines_until_clamp;
            let mut height = child_layout.size.height;
            if is_flow_block(doc, child_id)
                && let Some(offset) = self.container(child_id, top + child_top, None)
            {
                let new_height = self.shortened_height(child_id, offset, &child_layout);
                if new_height != height {
                    self.heights.push((child_id, new_height));
                    self.parents.push(node_id);
                    if ends_with_collapsing_margin(doc, child_id) {
                        self.reports.push(LINE_CLAMP_MARGIN);
                    }
                    if has_out_of_flow_descendant(doc, child_id) {
                        self.reports.push(LINE_CLAMP_OUT_OF_FLOW);
                    }
                    shift += height - new_height;
                    height = new_height;
                }
            }
            // A box that is not a line box directly after the clamp point
            // means content follows it.
            if before == 0 && self.lines_until_clamp == 0 {
                self.lines_until_clamp = -1;
            }
            if saved.is_none() && self.lines_until_clamp <= 0 {
                saved = Some(child_top + height);
            }
        }
        saved
    }

    /// Count the lines of the inline formatting context `node_id`, whose
    /// content box starts at `top` in the clamp root. Returns the bottom of
    /// its first line past the clamp point, in its content box, if any.
    fn lines(&mut self, node_id: NodeId, top: f32) -> Option<f32> {
        let doc = self.doc;
        let scale = doc.viewport.scale();
        let text = doc.nodes[node_id]
            .element_data()?
            .inline_layout_data
            .as_ref()?;
        self.ifcs.push(node_id);
        let mut saved = None;
        for index in 0..formatted_line_count(text) {
            let metrics = *text.layout.get(index)?.metrics();
            if self.lines_until_clamp == 1 {
                self.clamp = Some((node_id, index, top + metrics.baseline / scale));
            }
            self.lines_until_clamp -= 1;
            if saved.is_none() && self.lines_until_clamp <= 0 {
                saved = Some(metrics.block_max_coord / scale);
            }
        }
        saved
    }

    /// The border-box block size of a block that ends at `offset` (in its
    /// border box) after clamping: Chromium's legacy intrinsic block size
    /// (the offset plus block-end border and padding, without the end margin
    /// strut), unless its block size is fixed.
    fn shortened_height(&mut self, node_id: NodeId, offset: f32, layout: &Layout) -> f32 {
        let style = self.doc.nodes[node_id].layout_style();
        let pb_bottom = layout.padding.bottom + layout.border.bottom;
        let pb_sum = layout.padding.top + layout.border.top + pb_bottom;
        let intrinsic = offset + pb_bottom;
        let height = style.size().height;
        let min = style.min_size().height;
        let max = style.max_size().height;
        let fixed_min: Option<f32> = min.maybe_resolve(None::<f32>, resolve_calc_value);
        let fixed_max: Option<f32> = max.maybe_resolve(None::<f32>, resolve_calc_value);
        let fixed_height: Option<f32> = height.maybe_resolve(None::<f32>, resolve_calc_value);
        if (!height.is_auto() && fixed_height.is_none())
            || (!min.is_auto() && fixed_min.is_none())
            || (!max.is_auto() && fixed_max.is_none())
        {
            self.reports.push(LINE_CLAMP_PERCENT);
            return layout.size.height;
        }
        if !height.is_auto() || style.aspect_ratio().is_some() {
            return layout.size.height;
        }
        let adjustment = if style.box_sizing() == BoxSizing::ContentBox {
            pb_sum
        } else {
            0.0
        };
        intrinsic
            .maybe_clamp(
                fixed_min.maybe_add(adjustment),
                fixed_max.maybe_add(adjustment),
            )
            .max(pb_sum)
    }
}

/// Whether `node_id` is an in-flow block container of the formatting context
/// around it (Taffy's `is_in_same_bfc`), whose lines the clamp counts.
fn is_flow_block(doc: &BaseDocument, node_id: NodeId) -> bool {
    let node = &doc.nodes[node_id];
    if node.is_fieldset() || node.primary_styles().is_none() {
        return false;
    }
    if let Some(element) = node.element_data()
        && matches!(
            element.name.local.as_ref(),
            "input" | "textarea" | "select" | "button"
        )
    {
        return false;
    }
    let style = node.layout_style();
    let overflow = style.overflow();
    style.is_block()
        && !style.is_table()
        && !style.is_compressible_replaced()
        && !style.position().is_out_of_flow()
        && !overflow.x.is_scroll_container()
        && !overflow.y.is_scroll_container()
        && !style.contain().establishes_independent_formatting_context()
        && node
            .primary_styles()
            .is_some_and(|s| s.get_box().float == style::computed_values::float::T::None)
}

/// Whether a block's block-end margin collapses with that of its last in-flow
/// child, so that dropping its end margin strut would move later boxes.
fn ends_with_collapsing_margin(doc: &BaseDocument, node_id: NodeId) -> bool {
    let node = &doc.nodes[node_id];
    let layout = node.unrounded_layout();
    if layout.padding.bottom + layout.border.bottom > 0.0
        || node.flags.is_inline_root()
        || !node.layout_style().size().height.is_auto()
    {
        return false;
    }
    let children = node.layout_children.borrow();
    let last = children.iter().flatten().copied().rev().find(|&id| {
        let child = &doc.nodes[id];
        child.primary_styles().is_some_and(|s| {
            !s.get_box().display.is_none() && !s.get_box().position.is_absolutely_positioned()
        })
    });
    let Some(last) = last else {
        return false;
    };
    doc.nodes[last].unrounded_layout().margin.bottom != 0.0
        || (is_flow_block(doc, last) && ends_with_collapsing_margin(doc, last))
}

/// Whether an absolutely positioned box is in the subtree of `node_id`.
fn has_out_of_flow_descendant(doc: &BaseDocument, node_id: NodeId) -> bool {
    let children = doc.nodes[node_id].layout_children.borrow();
    children.iter().flatten().any(|&id| {
        let child = &doc.nodes[id];
        child
            .primary_styles()
            .is_some_and(|s| s.get_box().position.is_absolutely_positioned())
            || (child.element_data().is_some() && has_out_of_flow_descendant(doc, id))
    })
}

#[cfg(test)]
#[path = "line_clamp_tests.rs"]
mod tests;
