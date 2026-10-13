mod background;
mod border;
mod box_shadow;
mod clip_path;
mod form_controls;
mod mask;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::kurbo_css::CssBox;
use crate::CustomWidgetSceneMap;
use crate::color::{Color, ToColorColor};
use crate::debug_overlay::render_debug_overlay;
use crate::filters::convert_filters;
use crate::kurbo_css::NonUniformRoundedRectRadii;
use crate::layers::LayerManager;
use crate::sizing::compute_object_fit;
use crate::text::DrawTextContext;
use anyrender::{PaintScene, Scene};
use blitz_dom::node::{
    ListItemLayout, ListItemLayoutPosition, Marker, NodeData, RasterImageData, TextInputData,
    TextNodeData,
};
use blitz_dom::{BaseDocument, ElementData, Node, NodeId, local_name};
use blitz_traits::devtools::DevtoolSettings;

use style::values::computed::{BorderCornerRadius, ColorOrAuto};
use style::{
    dom::TElement,
    properties::{
        ComputedValues, generated::longhands::visibility::computed_value::T as StyloVisibility,
        style_structs::Font,
    },
    values::{
        computed::{CSSPixelLength, Contain, Overflow},
        specified::box_::{DisplayInside, DisplayOutside},
        specified::image::ImageRendering,
    },
};

use kurbo::{self, Affine, Insets, Point, Rect, Shape, Size, Stroke, Vec2};
use peniko::{self, Fill, ImageData, ImageSampler};
use style::values::generics::color::GenericColor;
use taffy::Layout;

// Default light control-selection palette; document selection has its own painter.
const CONTROL_SELECTION_BACKGROUND: Color = Color::from_rgb8(51, 103, 209);
const CONTROL_SELECTION_FOREGROUND: Color = Color::WHITE;

/// A short-lived struct which holds a bunch of parameters for rendering a scene so
/// that we don't have to pass them down as parameters
pub struct BlitzDomPainter<'dom, 'a> {
    /// Input parameters (read only) for generating the Scene
    pub(crate) dom: &'dom BaseDocument,
    pub(crate) scale: f64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) initial_x: f64,
    pub(crate) initial_y: f64,
    /// The id of the document's root element (cached to avoid re-resolving it for every element)
    pub(crate) root_element_id: Option<NodeId>,
    /// Scrollbar hover/drag state, resolved once per scene like the root element
    #[cfg(feature = "scrollbars")]
    pub(crate) hovered_scrollbar: Option<blitz_dom::node::ScrollbarRef>,
    #[cfg(feature = "scrollbars")]
    pub(crate) scrollbar_drag_target: Option<blitz_dom::node::ScrollbarRef>,
    pub(crate) layer_manager: LayerManager,
    /// Reusable scratch allocations shared by all text layouts painted for this document.
    pub(crate) draw_text_context: RefCell<DrawTextContext>,
    /// Ordinary collapsed-table cell backgrounds painted in the table's
    /// background phase, before shared borders and cell foreground content.
    pub(crate) table_backgrounds: RefCell<HashSet<NodeId>>,
    /// Cached selection ranges for O(1) lookup: node_id -> (start_offset, end_offset)
    pub(crate) selection_ranges: HashMap<NodeId, (usize, usize)>,
    /// The ancestors of elements with a `mix-blend-mode` other than `normal`:
    /// those that are stacking contexts isolate the blending in a layer.
    pub(crate) blend_ancestors: HashSet<NodeId>,

    // Pre-computed `Scene`s for each CustomWidget
    pub(crate) custom_widget_scenes: &'a CustomWidgetSceneMap,
}

impl<'dom, 'a> BlitzDomPainter<'dom, 'a> {
    /// Create a new BlitzDomPainter for the given document
    pub fn new(
        dom: &'dom BaseDocument,
        scale: f64,
        width: u32,
        height: u32,
        initial_x: f64,
        initial_y: f64,
        custom_widget_scenes: &'a CustomWidgetSceneMap,
    ) -> Self {
        let selection_ranges: HashMap<NodeId, (usize, usize)> = dom
            .get_text_selection_ranges()
            .into_iter()
            .map(|(node_id, start, end)| (node_id, (start, end)))
            .collect();

        let layer_manager = LayerManager::default();
        let root_element_id = dom.try_root_element().map(|el| el.id);
        let blend_ancestors = blend_ancestors(dom, root_element_id);

        Self {
            dom,
            scale,
            width,
            height,
            initial_x,
            initial_y,
            root_element_id,
            #[cfg(feature = "scrollbars")]
            hovered_scrollbar: dom.hovered_scrollbar(),
            #[cfg(feature = "scrollbars")]
            scrollbar_drag_target: dom.scrollbar_drag_target(),
            layer_manager,
            draw_text_context: RefCell::new(DrawTextContext::default()),
            table_backgrounds: RefCell::new(HashSet::new()),
            selection_ranges,
            blend_ancestors,
            custom_widget_scenes,
        }
    }

    /// Draw the current tree to current render surface
    /// Eventually we'll want the surface itself to be passed into the render function, along with things like the viewport
    ///
    /// This assumes styles are resolved and layout is complete.
    /// Make sure you do those before trying to render
    pub fn paint_scene(&self, scene: &mut impl PaintScene) {
        if self.dom.has_pending_critical_resources() {
            return;
        }

        // Simply render the document (the root element (note that this is not the same as the root node)))
        // scene.reset();
        let viewport_scroll = self.dom.as_ref().viewport_scroll();

        // A document without a root element (e.g. an empty iframe sub-document) has
        // nothing to paint.
        let Some(root_element) = self.dom.as_ref().try_root_element() else {
            return;
        };
        let root_id = root_element.id;
        let bg_width = (self.width as f32).max(root_element.final_layout().size.width);
        let bg_height = (self.height as f32).max(root_element.final_layout().size.height);

        let background_color = {
            let html_color = root_element
                .primary_styles()
                .map(|s| s.clone_background_color())
                .unwrap_or(GenericColor::TRANSPARENT_BLACK);
            if html_color == GenericColor::TRANSPARENT_BLACK {
                root_element
                    .children
                    .iter()
                    .find_map(|id| {
                        self.dom
                            .as_ref()
                            .get_node(*id)
                            .filter(|node| node.data.is_element_with_tag_name(&local_name!("body")))
                    })
                    .and_then(|body| body.primary_styles())
                    .map(|style| {
                        let current_color = style.clone_color();
                        style
                            .clone_background_color()
                            .resolve_to_absolute(&current_color)
                    })
            } else {
                let current_color = root_element.primary_styles().unwrap().clone_color();
                Some(html_color.resolve_to_absolute(&current_color))
            }
        };

        if let Some(bg_color) = background_color {
            let bg_color = bg_color.as_srgb_color();
            let rect = Rect::from_origin_size(
                (self.initial_x, self.initial_y),
                (bg_width as f64, bg_height as f64),
            );
            scene.fill(Fill::NonZero, Affine::IDENTITY, bg_color, None, &rect);
        }

        // The root clip rectangle is the viewport (in screen coordinates, with the
        // initial offset already subtracted). Elements outside of this are culled, and
        // scrollports narrow this rectangle further for their descendants.
        let viewport_clip_rect = Rect::new(0.0, 0.0, self.width as f64, self.height as f64);

        self.render_element(
            scene,
            root_id,
            Affine::translate(Vec2 {
                x: self.initial_x - (viewport_scroll.x * self.scale),
                y: self.initial_y - (viewport_scroll.y * self.scale),
            }),
            viewport_clip_rect,
        );

        // Render debug overlay
        if self.dom.devtools().highlight_hover {
            if let Some(node_id) = self.dom.as_ref().get_hover_node_id() {
                render_debug_overlay(
                    scene,
                    self.dom,
                    node_id,
                    self.scale,
                    self.initial_x,
                    self.initial_y,
                );
            }
        }
        if let Some(node_id) = self.dom.devtools().highlight_node {
            // Only nodes with element data have a layout to visualise
            if self
                .dom
                .as_ref()
                .get_node(node_id)
                .is_some_and(|node| node.element_data().is_some())
            {
                render_debug_overlay(
                    scene,
                    self.dom,
                    node_id,
                    self.scale,
                    self.initial_x,
                    self.initial_y,
                );
            }
        }
    }

    /// Renders a node, but is guaranteed that the node is an element
    /// This is because the font_size is calculated from layout resolution and all text is rendered directly here, instead
    /// of a separate text stroking phase.
    ///
    /// In Blitz, text styling gets its attributes from its container element/resolved styles
    /// In other libraries, text gets its attributes from a `text` element - this is not how HTML works.
    ///
    /// Approaching rendering this way guarantees we have all the styles we need when rendering text with not having
    /// to traverse back to the parent for its styles, or needing to pass down styles
    fn render_element(
        &self,
        scene: &mut impl PaintScene,
        node_id: NodeId,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        let node = &self.dom.as_ref().tree()[node_id];

        // Early return if the element is hidden
        if matches!(node.taffy_display(), taffy::Display::None) {
            return;
        }

        // Only draw elements with a style
        let Some(styles) = node.primary_styles() else {
            return;
        };

        // Hide inputs with type=hidden
        // Implemented here rather than using the style engine for performance reasons
        if node.local_name() == "input" && node.attr(local_name!("type")) == Some("hidden") {
            return;
        }

        // Hide elements with a visibility style other than visible
        if styles.get_inherited_box().visibility != StyloVisibility::Visible {
            return;
        }

        let effects = styles.get_effects();
        let opacity = effects.opacity;
        if opacity == 0.0 {
            return;
        }
        let has_opacity = opacity < 1.0;

        // A single-line text input's text is clipped to its content box (its
        // inner editor's box); a text area is a scroll container clipped at
        // its padding box, as in Chromium.
        let single_line_input = node
            .element_data()
            .and_then(|el| el.text_input_data())
            .is_some_and(|input| !input.is_multiline);
        // Hit testing clips with the same predicate (`Node::clips_overflow`).
        let should_clip = node.clips_overflow();

        // Apply padding/border offset to inline root
        let taffy::Layout {
            size,
            border,
            padding,
            ..
        } = *node.final_layout();
        let location = node.visual_location();
        let box_position = Vec2::new(location.x as f64, location.y as f64) * self.scale;
        let box_size = Size::new(size.width as f64, size.height as f64);
        let border_box = Rect::from_origin_size(box_position.to_point(), box_size);
        let scaled_pb = (padding + border).map(f64::from);
        let content_position = kurbo::Point {
            x: scaled_pb.left,
            y: scaled_pb.top,
        };
        let content_box_size = kurbo::Size {
            width: (size.width as f64 - scaled_pb.left - scaled_pb.right) * self.scale,
            height: (size.height as f64 - scaled_pb.top - scaled_pb.bottom) * self.scale,
        };

        // Don't render things that are out of view
        let overflow = *node.scrollable_overflow();
        let transform = parent_style_transform
            * Affine::translate(box_position)
            * node.transform().as_deref().copied().unwrap_or_default();

        let screen_transform = Affine::translate(Vec2 {
            x: -self.initial_x,
            y: -self.initial_y,
        }) * transform;
        let screen_bbox = screen_transform.transform_rect_bbox(overflow.union(border_box));

        // Cull elements that fall entirely outside the current clip rectangle. In addition to
        // the viewport, `clip_rect` is narrowed by any ancestor scrollport (see below), so this
        // also culls elements scrolled out of view inside a clipping/scrolling container.
        if screen_bbox.x1 < clip_rect.x0
            || screen_bbox.x0 > clip_rect.x1
            || screen_bbox.y1 < clip_rect.y0
            || screen_bbox.y0 > clip_rect.y1
        {
            return;
        }

        // Cull elements whose transformed geometry is not representable in f32 (e.g.
        // `transform: scale(99e99)`): the renderer works in f32 and flattening such paths
        // can attempt to allocate an unbounded number of line segments.
        let f32_max = f64::from(f32::MAX);
        if !screen_bbox.is_finite()
            || [
                screen_bbox.x0,
                screen_bbox.y0,
                screen_bbox.x1,
                screen_bbox.y1,
            ]
            .iter()
            .any(|c| c.abs() > f32_max)
        {
            return;
        }

        // Optimise zero-area (/very small area) clips by not rendering at all
        let clip_area = content_box_size.width * content_box_size.height;
        let overflow_area =
            node.scrollable_overflow().width() * node.scrollable_overflow().height();
        if should_clip && clip_area < 0.01 && overflow_area < 0.01 {
            return;
        }

        #[cfg(feature = "custom-widget")]
        let custom_widget_scene = self.custom_widget_scenes.get(&(self.dom.id(), node_id));
        #[cfg(not(feature = "custom-widget"))]
        let custom_widget_scene = None;

        // Apply CSS transform property (where transforms are 2d)

        let mut decoration_layout = *node.final_layout();
        let mut decoration_transform = transform;
        if let Some(element) = node.element_data()
            && let blitz_dom::node::SpecialElementData::TableRoot(table) = &element.special_data
        {
            let grid = table.grid_rect.borrow();
            decoration_layout.size = grid.size;
            decoration_transform *= Affine::translate((0.0, grid.location.y as f64 * self.scale));
        }
        let mut cx = self.element_cx(
            node,
            decoration_layout,
            decoration_transform,
            custom_widget_scene,
        );
        let mut fieldset = cx.fieldset_decorations();

        // If this element clips its overflow it establishes a scrollport: narrow the clip
        // rectangle passed to descendants to the visible (clipped) region so that content
        // scrolled out of view is culled rather than drawn and clipped away. The box used
        // here matches the clip applied to the content below.
        let child_clip_rect = if should_clip {
            let clip_box = if single_line_input {
                cx.frame.content_box_path()
            } else {
                cx.frame.padding_box_path()
            };
            clip_rect.intersect(screen_transform.transform_rect_bbox(clip_box.bounding_box()))
        } else {
            clip_rect
        };

        // Compute clip-path (if any) and wrap all rendering in a clip layer
        let clip_path_shape = cx.clip_path_shape();
        let has_clip_path = clip_path_shape.is_some();
        let default_clip = cx.frame.border_box_path();
        let mut clip_path_for_layer = clip_path_shape.unwrap_or(default_clip);
        clip_path_for_layer.apply_affine(Affine::scale(self.scale));

        // CSS 2 `clip` property (absolutely positioned elements only). Applies to the whole of
        // the element's rendering (including outlines and shadows), so it is the outermost layer.
        let css_clip_rect = cx.css_clip_rect();
        if css_clip_rect.is_some_and(|rect| rect.is_zero_area()) {
            return;
        }

        // `mix-blend-mode` (Compositing 1): the element's whole rendering is
        // drawn in a layer blended with what its isolated group has painted
        // behind it, also outside the clips of ancestors that do not isolate
        // (`overflow`, `clip`): those are closed around the blend layer and
        // drawn again inside it. Chromium ignores the property on the root
        // element, the root group. A stacking context with a blended
        // element inside isolates it in a layer of its own (the effect,
        // clip-path and mask layers below are isolated groups as well). Both
        // layers cover the viewport: content outside it is not drawn.
        let is_root = Some(node_id) == self.root_element_id;
        let blend = if is_root {
            None
        } else {
            crate::blend::mix_blend_mode(effects.mix_blend_mode)
        };
        let isolate = blend.is_none()
            && !is_root
            && self.blend_ancestors.contains(&node_id)
            && node.is_stacking_context_root(is_flex_or_grid_item(self.dom, node));
        let group_transform = Affine::translate((self.initial_x, self.initial_y));
        let group_bounds = Rect::new(0.0, 0.0, self.width as f64, self.height as f64);
        let mut suspended_clips = None;
        if let Some(mode) = blend {
            let clips = self.layer_manager.suspend_backdrop_clips(scene);
            scene.push_layer(
                Fill::NonZero,
                mode,
                1.0,
                group_transform,
                &group_bounds,
                None,
                None,
            );
            self.layer_manager.begin_group();
            self.layer_manager.resume_backdrop_clips(scene, &clips);
            suspended_clips = Some(clips);
        } else if isolate {
            scene.push_layer(
                Fill::NonZero,
                peniko::Mix::Normal,
                1.0,
                group_transform,
                &group_bounds,
                None,
                None,
            );
            self.layer_manager.begin_group();
        }

        let css_clip_layer_pushed = self.layer_manager.maybe_push_backdrop_clip(
            scene,
            css_clip_rect.is_some(),
            cx.transform,
            &css_clip_rect.unwrap_or(Rect::ZERO),
        );

        let current_color = cx.style.clone_color();
        let filter = convert_filters(&effects.filter.0, &current_color).map(Arc::new);
        let backdrop_filter =
            convert_filters(&effects.backdrop_filter.0, &current_color).map(Arc::new);

        // Opacity and filters apply to the element's whole rendering, its
        // outline and outer box shadows included: they are drawn inside the
        // effect layer when there is one.
        let decorations_in_effect_layer = has_opacity || filter.is_some();
        if !decorations_in_effect_layer {
            cx.swap_fieldset_frame(&mut fieldset);
            cx.draw_outset_box_shadow(scene);
            cx.swap_fieldset_frame(&mut fieldset);
        }

        // clip-path clip ayer
        self.layer_manager.maybe_with_layer(
            scene,
            has_clip_path,
            1.0,
            cx.transform,
            &clip_path_for_layer,
            None,
            None,
            |scene| {
                if has_clip_path {
                    self.layer_manager.begin_group();
                }
                // If the element has a CSS `mask`, then push an isolation layer for the
                // masked content. The mask is applied when the layer is popped below.
                let mask_layer_pushed = cx.maybe_push_css_mask_layer(scene);
                if mask_layer_pushed {
                    self.layer_manager.begin_group();
                }
                // `cx.transform` is mutated to apply scroll offsets while drawing content.
                // Save it so that the mask can be drawn untransformed by scroll offsets.
                let unscrolled_transform = cx.transform;

                // Adjust effect layer clip by filter expansion area
                //
                // Returns a rectangle centered at the origin representing how much the filter
                // expands the processing region in each direction. The rect coordinates are:
                // - x0: negative left expansion
                // - y0: negative top expansion
                // - x1: positive right expansion
                // - y1: positive bottom expansion
                let filter_expansion_area = filter
                    .as_ref()
                    .map(|f| f.expansion_rect())
                    .unwrap_or(Rect::ZERO);

                // The effect layer covers what the element paints: its border box and,
                // when they are drawn in the layer, its outline, outer box shadows and
                // content overflowing the border box (unless the element clips it).
                let mut effect_layer_clip = cx.frame.border_box_path().bounding_box();
                if decorations_in_effect_layer {
                    effect_layer_clip = effect_layer_clip
                        .union(cx.outline_extent())
                        .union(cx.outset_box_shadow_extent());
                    if !should_clip {
                        effect_layer_clip = effect_layer_clip.union(*node.scrollable_overflow());
                    }
                }
                effect_layer_clip.x0 += filter_expansion_area.x0;
                effect_layer_clip.y0 += filter_expansion_area.y0;
                effect_layer_clip.x1 += filter_expansion_area.x1;
                effect_layer_clip.y1 += filter_expansion_area.y1;

                // A backdrop filter without a filter is drawn in a layer of its own,
                // clipped to the border box with its rounded corners; the element's
                // content is not clipped by it. With a filter as well, both stay in
                // one layer clipped as above.
                let separate_backdrop = filter.is_none() && backdrop_filter.is_some();
                let (layer_backdrop, own_backdrop) = if separate_backdrop {
                    (None, backdrop_filter)
                } else {
                    (backdrop_filter, None)
                };

                // Opacity/Filter layer if box has opacity or a filter.
                let effect_layer = has_opacity || filter.is_some() || layer_backdrop.is_some();
                self.layer_manager.maybe_with_layer(
                    scene,
                    effect_layer,
                    opacity,
                    cx.transform,
                    &effect_layer_clip,
                    filter,
                    layer_backdrop,
                    |scene| {
                        if effect_layer {
                            self.layer_manager.begin_group();
                        }
                        if decorations_in_effect_layer {
                            cx.swap_fieldset_frame(&mut fieldset);
                            cx.draw_outset_box_shadow(scene);
                            cx.swap_fieldset_frame(&mut fieldset);
                        }
                        if own_backdrop.is_some() {
                            self.layer_manager.maybe_with_layer(
                                scene,
                                true,
                                1.0,
                                cx.transform,
                                &cx.frame.border_box_path(),
                                None,
                                own_backdrop,
                                |_| {},
                            );
                        }
                        cx.swap_fieldset_frame(&mut fieldset);
                        cx.draw_background(scene);
                        cx.draw_inset_box_shadow(scene);
                        cx.draw_table_row_backgrounds(scene);
                        cx.draw_table_cell_backgrounds(scene);
                        cx.draw_table_borders(scene);
                        // A fieldset's border is not painted behind its legend.
                        let no_clip = kurbo::BezPath::new();
                        let border_clip = fieldset.as_ref().map(|f| &f.border_clip);
                        let border_clipped = self.layer_manager.maybe_push_layer(
                            scene,
                            border_clip.is_some(),
                            1.0,
                            cx.transform,
                            border_clip.unwrap_or(&no_clip),
                            None,
                            None,
                        );
                        cx.draw_border(scene);
                        self.layer_manager.maybe_pop_layer(scene, border_clipped);
                        cx.swap_fieldset_frame(&mut fieldset);
                        cx.stroke_devtools(scene);

                        // TODO: allow layers with opacity to be unclipped (overflow: visible)
                        let clip = if single_line_input {
                            &cx.frame.content_box_path()
                        } else {
                            &cx.frame.padding_box_path()
                        };

                        // Clip layer if box requires clipping. An overflow
                        // clip does not isolate blending.
                        let clipped = self.layer_manager.maybe_push_backdrop_clip(
                            scene,
                            should_clip,
                            cx.transform,
                            clip,
                        );
                        {
                            // Now that background has been drawn, offset pos and cx in order to draw our contents scrolled
                            let content_position = Point {
                                x: content_position.x - node.scroll_offset().x,
                                y: content_position.y - node.scroll_offset().y,
                            };

                            cx.transform = transform.then_translate(Vec2 {
                                x: -node.scroll_offset().x * self.scale,
                                y: -node.scroll_offset().y * self.scale,
                            });
                            cx.draw_image(scene);
                            #[cfg(feature = "svg")]
                            cx.draw_svg(scene);
                            #[cfg(feature = "custom-widget")]
                            cx.draw_custom_widget(scene);
                            cx.draw_sub_document(scene);
                            cx.draw_input(scene);
                            cx.draw_text_input_text(scene, content_position);
                            cx.draw_inline_layout(scene, content_position);
                            cx.draw_marker(scene, content_position);
                            cx.draw_children(scene, cx.transform, child_clip_rect);
                        }
                        self.layer_manager.maybe_pop_backdrop_clip(scene, clipped);

                        // Outlines cover the border/content and remain unscrolled.
                        cx.transform = unscrolled_transform;
                        cx.draw_outline(scene);

                        // Overlay scrollbars, drawn unscrolled above the
                        // clipped content.
                        #[cfg(feature = "scrollbars")]
                        {
                            cx.transform = unscrolled_transform;
                            cx.draw_scrollbars(scene);
                        }
                        if effect_layer {
                            self.layer_manager.end_group();
                        }
                    },
                );

                // Apply the CSS `mask` (if any) to the content drawn above
                cx.transform = unscrolled_transform;
                if mask_layer_pushed {
                    self.layer_manager.end_group();
                }
                cx.maybe_pop_css_mask_layer(scene, mask_layer_pushed);
                if has_clip_path {
                    self.layer_manager.end_group();
                }
            },
        );

        self.layer_manager
            .maybe_pop_backdrop_clip(scene, css_clip_layer_pushed);

        if let Some(clips) = suspended_clips {
            // Close the clips drawn again inside the blend layer, blend it,
            // and reopen them for what the group paints next.
            self.layer_manager.suspend_backdrop_clips(scene);
            self.layer_manager.end_group();
            scene.pop_layer();
            self.layer_manager.resume_backdrop_clips(scene, &clips);
        } else if isolate {
            self.layer_manager.end_group();
            scene.pop_layer();
        }
    }

    fn render_node(
        &self,
        scene: &mut impl PaintScene,
        node_id: NodeId,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        let node = &self.dom.as_ref().tree()[node_id];

        match &node.data {
            NodeData::Element(_) | NodeData::AnonymousBlock(_) => {
                self.render_element(scene, node_id, parent_style_transform, clip_rect)
            }
            NodeData::Text(TextNodeData { .. }) => {
                // Text nodes should never be rendered directly
                // (they should always be rendered as part of an inline layout)
                // unreachable!()
            }
            NodeData::Document(_) => {}
            // NodeData::Doctype => {}
            NodeData::Comment { .. } => {} // NodeData::ProcessingInstruction { .. } => {}
        }
    }

    fn element_cx(
        &'dom self,
        node: &'dom Node,
        layout: Layout,
        transform: Affine,
        custom_widget_scene: Option<&'a Scene>,
    ) -> ElementCx<'dom, 'a> {
        let style = node
            .stylo_element_data()
            .primary_styles()
            .as_ref()
            .map(|styles| (*styles).clone())
            .unwrap_or_else(|| {
                ComputedValues::initial_values_with_font_override(Font::initial_values()).to_arc()
            });

        let scale = self.scale;

        // todo: maybe cache this so we don't need to constantly be figuring it out
        // It is quite a bit of math to calculate during render/traverse
        // Also! we can cache the bezpaths themselves, saving us a bunch of work
        let frame = create_css_rect(&style, &layout, scale);

        let element = node.element_data().unwrap();

        ElementCx {
            context: self,
            frame,
            scale,
            style,
            node,
            element,
            transform,
            #[cfg(feature = "svg")]
            svg: element.svg_data(),
            text_input: element.text_input_data(),
            list_item: element.list_item_data.as_deref(),
            devtools: self.dom.devtools(),
            custom_widget_scene,
        }
    }
}

/// The ancestors (in the layout tree) of the elements other than the root
/// with a `mix-blend-mode` other than `normal`.
fn blend_ancestors(dom: &BaseDocument, root_element_id: Option<NodeId>) -> HashSet<NodeId> {
    use style::computed_values::mix_blend_mode::T as MixBlendMode;
    let tree = dom.tree();
    let mut ancestors = HashSet::new();
    for (id, node) in tree.iter() {
        if Some(id) == root_element_id
            || node
                .primary_styles()
                .is_none_or(|style| style.get_effects().mix_blend_mode == MixBlendMode::Normal)
        {
            continue;
        }
        let mut current = node.layout_parent.get().or(node.parent);
        while let Some(id) = current {
            if !ancestors.insert(id) {
                break;
            }
            let ancestor = &tree[id];
            current = ancestor.layout_parent.get().or(ancestor.parent);
        }
    }
    ancestors
}

/// Whether the node is a flex or grid item (a `z-index` then makes it a
/// stacking context without positioning).
fn is_flex_or_grid_item(dom: &BaseDocument, node: &Node) -> bool {
    node.layout_parent
        .get()
        .and_then(|parent| dom.get_node(parent))
        .and_then(|parent| parent.primary_styles())
        .is_some_and(|style| {
            matches!(
                style.clone_display().inside(),
                DisplayInside::Flex | DisplayInside::Grid
            )
        })
}

fn to_image_quality(image_rendering: ImageRendering) -> peniko::ImageQuality {
    match image_rendering {
        ImageRendering::Auto => peniko::ImageQuality::Medium,
        ImageRendering::CrispEdges => peniko::ImageQuality::Low,
        ImageRendering::Pixelated => peniko::ImageQuality::Low,
    }
}

/// Ensure that the `resized_image` field has a correctly sized image
fn to_peniko_image(image: &RasterImageData, quality: peniko::ImageQuality) -> peniko::ImageBrush {
    peniko::ImageBrush {
        image: ImageData {
            data: image.data.clone(),
            format: peniko::ImageFormat::Rgba8,
            width: image.width,
            height: image.height,
            alpha_type: peniko::ImageAlphaType::Alpha,
        },
        sampler: ImageSampler {
            x_extend: peniko::Extend::Repeat,
            y_extend: peniko::Extend::Repeat,
            quality,
            alpha: 1.0,
        },
    }
}

/// A context of loaded and hot data to draw the element from
struct ElementCx<'dom, 'a> {
    context: &'dom BlitzDomPainter<'dom, 'a>,
    frame: CssBox,
    style: style::servo_arc::Arc<ComputedValues>,
    scale: f64,
    node: &'dom Node,
    element: &'dom ElementData,
    transform: Affine,
    #[cfg(feature = "svg")]
    svg: Option<&'dom usvg::Tree>,
    text_input: Option<&'dom TextInputData>,
    list_item: Option<&'dom ListItemLayout>,
    devtools: &'dom DevtoolSettings,
    #[cfg_attr(not(feature = "custom-widget"), expect(unused))]
    custom_widget_scene: Option<&'a Scene>,
}

/// The box decorations of a `<fieldset>` with a rendered legend
/// (<https://html.spec.whatwg.org/multipage/rendering.html#the-fieldset-and-legend-elements>),
/// as Chromium's `FieldsetPainter` paints them: shadows, background and
/// border are painted in the border box shortened at the top by half the
/// height the legend's border box exceeds the block-start border by, so that
/// the border runs through the legend's centre, and the border is clipped
/// out behind the legend (its border box horizontally, snapped to device
/// pixels, through the whole border thickness).
struct FieldsetDecorations {
    frame: CssBox,
    border_clip: kurbo::BezPath,
}

/// The part of a fieldset's border not painted behind its legend, in device
/// pixels relative to the fieldset's border box: the legend's border box
/// (`legend`, CSS pixels) horizontally, from the top through `border_bottom`
/// (the block-end edge of the painted block-start border) or the legend's
/// block end, whichever is lower. Its edges are snapped to device pixels
/// (`origin`: the device offset of the border box) as Chromium snaps them.
fn fieldset_border_cutout(
    legend: Rect,
    border_bottom: f64,
    scale: f64,
    origin: (f64, f64),
) -> Rect {
    let snap = |v: f64, t: f64| (v * scale + t).round() - t;
    Rect::new(
        snap(legend.x0, origin.0),
        snap(0.0, origin.1),
        snap(legend.x1, origin.0),
        snap(legend.y1.max(border_bottom), origin.1),
    )
}

/// Converts parley BoundingBox into peniko Rect
fn convert_rect(rect: &parley::BoundingBox) -> kurbo::Rect {
    peniko::kurbo::Rect::new(rect.x0, rect.y0, rect.x1, rect.y1)
}

impl ElementCx<'_, '_> {
    fn fieldset_decorations(&self) -> Option<FieldsetDecorations> {
        let legend_id = self.node.rendered_legend()?;
        let legend = self.context.dom.as_ref().tree().get(legend_id)?;
        let legend_layout = *legend.final_layout();
        let layout = *self.node.final_layout();
        let scale = self.scale;
        let border_top = layout.border.top as f64;
        let legend_height = legend_layout.size.height as f64;
        let outset = ((legend_height - border_top) / 2.0).max(0.0);

        let mut decoration_layout = layout;
        decoration_layout.size.height = (layout.size.height as f64 - outset).max(0.0) as f32;
        let mut frame = create_css_rect(&self.style, &decoration_layout, scale);
        let shift = Vec2::new(0.0, outset * scale);
        frame.border_box = frame.border_box + shift;
        frame.padding_box = frame.padding_box + shift;
        frame.content_box = frame.content_box + shift;
        frame.outline_box = frame.outline_box + shift;

        let [_, _, _, _, tx, ty] = self.transform.as_coeffs();
        let legend_rect = Rect::new(
            legend_layout.location.x as f64,
            legend_layout.location.y as f64,
            (legend_layout.location.x + legend_layout.size.width) as f64,
            legend_layout.location.y as f64 + legend_height,
        );
        let cutout = fieldset_border_cutout(legend_rect, outset + border_top, scale, (tx, ty));
        let outer = Rect::new(
            0.0,
            0.0,
            layout.size.width as f64 * scale,
            layout.size.height as f64 * scale,
        );
        let mut border_clip = kurbo::BezPath::new();
        // The outer rectangle clockwise, the cutout anticlockwise: a hole
        // under the non-zero rule.
        border_clip.move_to((outer.x0, outer.y0));
        border_clip.line_to((outer.x1, outer.y0));
        border_clip.line_to((outer.x1, outer.y1));
        border_clip.line_to((outer.x0, outer.y1));
        border_clip.close_path();
        border_clip.move_to((cutout.x0, cutout.y0));
        border_clip.line_to((cutout.x0, cutout.y1));
        border_clip.line_to((cutout.x1, cutout.y1));
        border_clip.line_to((cutout.x1, cutout.y0));
        border_clip.close_path();
        Some(FieldsetDecorations { frame, border_clip })
    }

    /// Exchange the frame with a fieldset's decoration frame (and back).
    fn swap_fieldset_frame(&mut self, fieldset: &mut Option<FieldsetDecorations>) {
        if let Some(fieldset) = fieldset {
            std::mem::swap(&mut self.frame, &mut fieldset.frame);
        }
    }

    /// Paint overlay scrollbar thumbs for scroll containers: `overflow:
    /// scroll`, or `auto` when the content overflows (never `hidden`/`clip`,
    /// which scroll only programmatically). Thumbs appear on scroll and fade
    /// out after a delay ([`BaseDocument::scrollbar_opacity`]); never-scrolled
    /// containers paint nothing, keeping thumbs out of static reftest
    /// screenshots.
    ///
    /// Geometry comes from [`Node::scrollbar_thumb`], shared with the
    /// thumb-drag hit testing in blitz-dom.
    #[cfg(feature = "scrollbars")]
    fn draw_scrollbars(&self, scene: &mut impl PaintScene) {
        // css-scrollbars-1 scrollbar-color: author thumb/track colors
        use blitz_dom::node::{ScrollbarColor, ScrollbarRef};
        use taffy::AbsoluteAxis;
        let (custom_thumb, custom_track) = match self.node.scrollbar_color() {
            ScrollbarColor::Auto => (None, None),
            ScrollbarColor::Colors { thumb, track } => {
                (Some(thumb.as_srgb_color()), Some(track.as_srgb_color()))
            }
        };

        let drag_target = self.context.scrollbar_drag_target;
        let hovered_thumb = self.context.hovered_scrollbar;

        // scrollbar-color doesn't affect overlay visibility: persistence is
        // UA policy, not author styling.
        let node_id = self.node.id;
        let opacity = self.context.dom.scrollbar_opacity(node_id);
        if opacity == 0.0 {
            return;
        }

        // Default thumb palette for the used color scheme; thumbs paint as
        // fill plus a thin contrast stroke so they read over same-colored
        // content.
        let dark_scheme =
            self.context.dom.viewport().color_scheme == blitz_traits::shell::ColorScheme::Dark;
        let (thumb_rest, thumb_hover, thumb_active, stroke_color) = if dark_scheme {
            (
                Color::from_rgba8(214, 214, 214, 178),
                Color::from_rgba8(190, 190, 190, 222),
                Color::from_rgba8(172, 172, 172, 255),
                Color::from_rgba8(0, 0, 0, 102),
            )
        } else {
            (
                Color::from_rgba8(128, 128, 128, 178),
                Color::from_rgba8(152, 152, 152, 222),
                Color::from_rgba8(170, 170, 170, 255),
                Color::from_rgba8(255, 255, 255, 102),
            )
        };

        // Chromium's hovered/pressed scrollbar contrast ratios.
        const HOVER_CONTRAST: f32 = 1.8;
        const ACTIVE_CONTRAST: f32 = 1.3;

        for axis in [AbsoluteAxis::Vertical, AbsoluteAxis::Horizontal] {
            if !self.node.wants_scrollbar(axis) {
                continue;
            }
            let Some(thumb) = self.node.scrollbar_thumb(axis) else {
                continue;
            };

            let rect = thumb.scale_from_origin(self.scale);

            // Track (only when the author specified a track color)
            if let Some(track_color) = custom_track {
                let padding_box = self.frame.padding_box;
                let track_rect = match axis {
                    AbsoluteAxis::Horizontal => {
                        Rect::new(padding_box.x0, rect.y0, padding_box.x1, rect.y1)
                    }
                    AbsoluteAxis::Vertical => {
                        Rect::new(rect.x0, padding_box.y0, rect.x1, padding_box.y1)
                    }
                };
                scene.fill(
                    Fill::NonZero,
                    self.transform,
                    track_color.multiply_alpha(opacity),
                    None,
                    &track_rect,
                );
            }

            let this = ScrollbarRef { node_id, axis };
            let is_active = drag_target == Some(this);
            let is_hovered = hovered_thumb == Some(this);
            let color = match custom_thumb {
                Some(base) if is_active => crate::color::blend_for_contrast(base, ACTIVE_CONTRAST),
                Some(base) if is_hovered => crate::color::blend_for_contrast(base, HOVER_CONTRAST),
                Some(base) => base,
                None if is_active => thumb_active,
                None if is_hovered => thumb_hover,
                None => thumb_rest,
            };
            let radius = match axis {
                AbsoluteAxis::Horizontal => rect.height() / 2.0,
                AbsoluteAxis::Vertical => rect.width() / 2.0,
            };
            scene.fill(
                Fill::NonZero,
                self.transform,
                color.multiply_alpha(opacity),
                None,
                &rect.to_rounded_rect(radius),
            );
            // Contrast stroke, default thumbs only: an author-specified
            // scrollbar-color is rendered exactly as given.
            if custom_thumb.is_none() {
                let stroke_width = self.scale;
                let stroke_rect = rect.inset(-stroke_width / 2.0);
                scene.stroke(
                    &Stroke::new(stroke_width),
                    self.transform,
                    stroke_color.multiply_alpha(opacity),
                    None,
                    &stroke_rect.to_rounded_rect(radius - stroke_width / 2.0),
                );
            }
        }
    }

    fn draw_inline_layout(&self, scene: &mut impl PaintScene, pos: Point) {
        if self.node.flags.is_inline_root() {
            let text_layout = self.element
                .inline_layout_data
                .as_ref()
                .unwrap_or_else(|| {
                    panic!("Tried to render node marked as inline root that does not have an inline layout: {:?}", self.node);
                });

            let transform =
                self.transform * Affine::translate((pos.x * self.scale, pos.y * self.scale));

            // Render inline element backgrounds (e.g. `<span style="background: ...">`)
            // behind the text and selection highlight.
            crate::text::draw_inline_backgrounds(
                scene,
                text_layout,
                self.context.dom,
                transform,
                self.node.id,
            );

            // Render text selection highlight (if any) using cached selection ranges
            if let Some(&(sel_start, sel_end)) = self.context.selection_ranges.get(&self.node.id) {
                crate::text::draw_text_selection(
                    scene,
                    &text_layout.layout,
                    transform,
                    sel_start,
                    sel_end,
                );
            }

            // `text-overflow: ellipsis` applies to the line boxes of a box that
            // clips its overflow; their end edge is the content box's.
            let ellipsis_edge = (self.node.clips_overflow()
                && matches!(
                    self.style.clone_text_overflow().second,
                    style::values::specified::text::TextOverflowSide::Ellipsis
                ))
            .then(|| (self.node.final_layout().content_box_width() as f64 * self.scale) as f32);

            // Render text
            let mut draw_text_context = self.context.draw_text_context.borrow_mut();
            crate::text::stroke_text(
                scene,
                &text_layout.layout,
                self.context.dom,
                transform,
                self.scale,
                self.node.id,
                &mut draw_text_context,
                None,
                ellipsis_edge,
                self.context.dom.line_clamp_ellipsis(self.node.id),
            );
        }
    }

    fn draw_text_input_text(&self, scene: &mut impl PaintScene, pos: Point) {
        // Render the text in text inputs
        if let Some(input_data) = self.text_input {
            // For single-line inputs, add an offset to vertically center the text input layout
            // within the content box of it's node.
            let y_offset = self.node.text_input_v_centering_offset(self.scale);
            let pos = Point {
                x: pos.x,
                y: pos.y + y_offset,
            };

            // Apply the scroll offset (stored in CSS pixels, scaled here to device pixels) so
            // that the caret stays visible. Single-line inputs scroll horizontally; multi-line
            // inputs scroll vertically.
            let scroll_offset = input_data.scroll_offset as f64 * self.scale;
            let (scroll_x, scroll_y) = if input_data.is_multiline {
                (0.0, scroll_offset)
            } else {
                (scroll_offset, 0.0)
            };

            let transform = self.transform
                * Affine::translate((pos.x * self.scale - scroll_x, pos.y * self.scale - scroll_y));

            // Render text
            let mut draw_text_context = self.context.draw_text_context.borrow_mut();
            match input_data.placeholder.as_ref() {
                Some(placeholder) if input_data.shows_placeholder() => crate::text::stroke_text(
                    scene,
                    &placeholder.layout,
                    self.context.dom,
                    transform,
                    self.scale,
                    self.node.id,
                    &mut draw_text_context,
                    Some(placeholder.color),
                    None,
                    None,
                ),
                _ => crate::text::stroke_text(
                    scene,
                    input_data.editor.try_layout().unwrap(),
                    self.context.dom,
                    transform,
                    self.scale,
                    self.node.id,
                    &mut draw_text_context,
                    None,
                    None,
                    None,
                ),
            }
            if self.node.is_focussed() {
                // Cover ordinary glyphs with an opaque background before drawing
                // the selected foreground; each glyph edge is antialiased once.
                let selection = input_data.editor.selection_geometry();
                if !selection.is_empty() {
                    let mut clip = kurbo::BezPath::new();
                    for (rect, _) in selection {
                        let rect = convert_rect(&rect);
                        scene.fill(
                            Fill::NonZero,
                            transform,
                            CONTROL_SELECTION_BACKGROUND,
                            None,
                            &rect,
                        );
                        clip.extend(rect.path_elements(0.1));
                    }
                    // One text pass for all selected lines (not one per rectangle).
                    scene.push_clip_layer(Fill::NonZero, transform, &clip);
                    crate::text::stroke_text(
                        scene,
                        input_data.editor.try_layout().unwrap(),
                        self.context.dom,
                        transform,
                        self.scale,
                        self.node.id,
                        &mut draw_text_context,
                        Some(CONTROL_SELECTION_FOREGROUND),
                        None,
                        None,
                    );
                    scene.pop_layer();
                }
                let editable = self.node.attr(local_name!("readonly")).is_none()
                    && self.node.attr(local_name!("disabled")).is_none();
                if editable && input_data.editor.raw_selection().is_collapsed() {
                    if let Some(cursor) = input_data.editor.cursor_geometry(self.scale as f32) {
                        let color = self.style.get_inherited_text().color;
                        let caret_color = match &self.style.get_inherited_ui().caret_color.0 {
                            ColorOrAuto::Auto => color,
                            ColorOrAuto::Color(caret_color) => {
                                caret_color.resolve_to_absolute(&color)
                            }
                        };
                        scene.fill(
                            Fill::NonZero,
                            transform,
                            caret_color.as_srgb_color(),
                            None,
                            &convert_rect(&cursor),
                        );
                    }
                }
            }
        }
    }

    fn draw_marker(&self, scene: &mut impl PaintScene, pos: Point) {
        if let Some(ListItemLayout {
            marker,
            position: ListItemLayoutPosition::Outside(layout),
        }) = self.list_item
        {
            // Right align and pad the bullet when rendering outside
            let x_padding = match marker {
                Marker::Char(_) => 8.0,
                Marker::String(_) => 0.0,
            };
            // Outside markers are placed outside the list item's border box
            // (`pos` is the origin of its content box)
            let item_layout = self.node.final_layout();
            let x_offset = -(layout.full_width() / layout.scale()
                + x_padding
                + item_layout.padding.left
                + item_layout.border.left);

            // Align the marker with the baseline of the first line of text in the list item
            let y_offset = if let Some(first_text_line) = &self
                .element
                .inline_layout_data
                .as_ref()
                .and_then(|text_layout| text_layout.layout.lines().next())
            {
                (first_text_line.metrics().baseline
                    - layout.lines().next().unwrap().metrics().baseline)
                    / layout.scale()
            } else {
                0.0
            };

            let pos = Point {
                x: pos.x + x_offset as f64,
                y: pos.y + y_offset as f64,
            };

            let transform =
                self.transform * Affine::translate((pos.x * self.scale, pos.y * self.scale));

            let mut draw_text_context = self.context.draw_text_context.borrow_mut();
            crate::text::stroke_text(
                scene,
                layout,
                self.context.dom,
                transform,
                self.scale,
                self.node.id,
                &mut draw_text_context,
                None,
                None,
                None,
            );
        }
    }

    fn draw_children(
        &self,
        scene: &mut impl PaintScene,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        // Negative z_index hoisted nodes

        if let Some(hoisted) = &self.node.stacking_context {
            for hoisted_child in hoisted.neg_z_hoisted_children() {
                self.draw_hoisted_child(scene, hoisted_child, parent_style_transform, clip_rect);
            }
        }

        // Regular children
        if let Some(children) = &*self.node.paint_children.borrow() {
            for child_id in children {
                // Fixed-position children of the root element are positioned against
                // the viewport and do not scroll with it, so cancel out the viewport
                // scroll (applied in `paint_scene`). A fixed box contained by a
                // transformed ancestor scrolls with it like any out-of-flow box.
                let child = &self.context.dom.as_ref().tree()[*child_id];
                let child_transform = if child.taffy_position() == taffy::Position::Fixed
                    && Some(self.node.id) == self.context.root_element_id
                {
                    let scroll = self.context.dom.as_ref().viewport_scroll();
                    parent_style_transform.pre_translate(kurbo::Vec2 {
                        x: scroll.x * self.scale,
                        y: scroll.y * self.scale,
                    })
                } else {
                    parent_style_transform
                };
                self.render_node(scene, *child_id, child_transform, clip_rect);
            }
        }

        // Positive z_index hoisted nodes
        if let Some(hoisted) = &self.node.stacking_context {
            for hoisted_child in hoisted.pos_z_hoisted_children() {
                self.draw_hoisted_child(scene, hoisted_child, parent_style_transform, clip_rect);
            }
        }
    }

    fn draw_hoisted_child(
        &self,
        scene: &mut impl PaintScene,
        child: &blitz_dom::HoistedPaintChild,
        parent_transform: Affine,
        mut clip_rect: Rect,
    ) {
        let tree = self.node.tree();
        let clips = child.clip_ancestors(tree, self.node.id);
        for id in &clips {
            let node = &tree[*id];
            let offset = blitz_dom::hoisted_child_position(tree, self.node.id, *id);
            let location = node.visual_location();
            let transform = parent_transform.pre_translate(Vec2::new(
                (offset.x + location.x) as f64 * self.scale,
                (offset.y + location.y) as f64 * self.scale,
            )) * node.transform().as_deref().copied().unwrap_or_default();
            let style = node.primary_styles().unwrap();
            let frame = create_css_rect(&style, node.final_layout(), self.scale);
            let clip = frame.padding_box_path();
            scene.push_clip_layer(peniko::Fill::NonZero, transform, &clip);
            self.context
                .layer_manager
                .record_backdrop_clip(transform, &clip);
            let screen = Affine::translate((-self.initial_x, -self.initial_y)) * transform;
            clip_rect = clip_rect.intersect(screen.transform_rect_bbox(clip.bounding_box()));
        }
        let offset = child.position(tree, self.node.id);
        self.render_node(
            scene,
            child.node_id,
            parent_transform.pre_translate(Vec2::new(
                offset.x as f64 * self.scale,
                offset.y as f64 * self.scale,
            )),
            clip_rect,
        );
        for _ in clips {
            self.context.layer_manager.forget_backdrop_clip();
            scene.pop_layer();
        }
    }

    #[cfg(feature = "svg")]
    fn draw_svg(&self, scene: &mut impl PaintScene) {
        use style::properties::generated::longhands::object_fit::computed_value::T as ObjectFit;

        let Some(svg) = self.svg else {
            return;
        };

        let width = self.frame.content_box.width() as u32;
        let height = self.frame.content_box.height() as u32;
        let svg_size = svg.size();

        let x = self.frame.content_box.origin().x;
        let y = self.frame.content_box.origin().y;

        // let object_fit = self.style.clone_object_fit();
        let object_position = self.style.clone_object_position();

        // Apply object-fit algorithm
        let container_size = taffy::Size {
            width: width as f32,
            height: height as f32,
        };
        let object_size = taffy::Size {
            width: svg_size.width(),
            height: svg_size.height(),
        };
        let paint_size = compute_object_fit(container_size, Some(object_size), ObjectFit::Contain);

        // Compute object-position
        let x_offset = object_position.horizontal.resolve(
            CSSPixelLength::new(container_size.width - paint_size.width) / self.scale as f32,
        ) * self.scale as f32;
        let y_offset = object_position.vertical.resolve(
            CSSPixelLength::new(container_size.height - paint_size.height) / self.scale as f32,
        ) * self.scale as f32;
        let x = x + x_offset.px() as f64;
        let y = y + y_offset.px() as f64;

        let x_scale = paint_size.width as f64 / object_size.width as f64;
        let y_scale = paint_size.height as f64 / object_size.height as f64;

        let transform = self
            .transform
            .pre_scale_non_uniform(x_scale, y_scale)
            .then_translate(Vec2 { x, y });

        anyrender_svg::render_svg_tree(scene, svg, transform);
    }

    fn draw_image(&self, scene: &mut impl PaintScene) {
        if let Some(image) = self.element.raster_image_data() {
            let width = self.frame.content_box.width() as u32;
            let height = self.frame.content_box.height() as u32;
            let x = self.frame.content_box.origin().x;
            let y = self.frame.content_box.origin().y;

            let object_fit = self.style.clone_object_fit();
            let object_position = self.style.clone_object_position();
            let image_rendering = self.style.clone_image_rendering();
            let quality = to_image_quality(image_rendering);

            // Apply object-fit algorithm
            let container_size = taffy::Size {
                width: width as f32,
                height: height as f32,
            };
            let object_size = taffy::Size {
                width: image.width as f32,
                height: image.height as f32,
            };
            let paint_size = compute_object_fit(container_size, Some(object_size), object_fit);

            // Compute object-position
            let x_offset = object_position.horizontal.resolve(
                CSSPixelLength::new(container_size.width - paint_size.width) / self.scale as f32,
            ) * self.scale as f32;
            let y_offset = object_position.vertical.resolve(
                CSSPixelLength::new(container_size.height - paint_size.height) / self.scale as f32,
            ) * self.scale as f32;
            let x = x + x_offset.px() as f64;
            let y = y + y_offset.px() as f64;

            let x_scale = paint_size.width as f64 / object_size.width as f64;
            let y_scale = paint_size.height as f64 / object_size.height as f64;
            let transform = self
                .transform
                .pre_translate(Vec2 { x, y })
                .pre_scale_non_uniform(x_scale, y_scale);

            scene.draw_image(to_peniko_image(image, quality).as_ref(), transform);
        }
    }

    #[cfg(feature = "custom-widget")]
    fn draw_custom_widget(&self, scene: &mut impl PaintScene) {
        if let Some(widget_scene) = self.custom_widget_scene {
            let x = self.frame.content_box.origin().x;
            let y = self.frame.content_box.origin().y;
            let transform = self.transform.then_translate(Vec2 { x, y });

            // TODO: eliminate clone
            scene.append_scene(widget_scene.clone(), transform);
        }
    }

    fn draw_sub_document(&self, scene: &mut impl PaintScene) {
        if let Some(sub_doc) = self.element.sub_doc_data().map(|doc| doc.inner()) {
            let scale = self.scale;
            let width = self.frame.content_box.width() as u32;
            let height = self.frame.content_box.height() as u32;

            // TODO: Support arbitrary transforms of subdocuments
            let translation = self.transform.translation();
            let initial_x = translation.x + self.frame.content_box.origin().x;
            let initial_y = translation.y + self.frame.content_box.origin().y;
            // let transform = self.transform.then_translate(Vec2 { x, y });

            let painter = BlitzDomPainter::new(
                &sub_doc,
                scale,
                width,
                height,
                initial_x,
                initial_y,
                self.custom_widget_scenes,
            );
            painter.paint_scene(scene);
        }
    }

    fn stroke_devtools(&self, scene: &mut impl PaintScene) {
        if self.devtools.show_layout {
            let shape = &self.frame.border_box;
            let stroke = Stroke::new(self.scale);

            let stroke_color = match self.node.taffy_display() {
                taffy::Display::Block | taffy::Display::FlowRoot => {
                    Color::new([1.0, 0.0, 0.0, 1.0])
                }
                taffy::Display::Flex => Color::new([0.0, 1.0, 0.0, 1.0]),
                taffy::Display::Grid => Color::new([0.0, 0.0, 1.0, 1.0]),
                taffy::Display::None => Color::new([0.0, 0.0, 1.0, 1.0]),
            };

            scene.stroke(&stroke, self.transform, stroke_color, None, &shape);
        }
    }
}
impl<'dom, 'a> std::ops::Deref for ElementCx<'dom, 'a> {
    type Target = BlitzDomPainter<'dom, 'a>;
    fn deref(&self) -> &Self::Target {
        self.context
    }
}

fn insets_from_taffy_rect(input: taffy::Rect<f64>) -> Insets {
    Insets {
        x0: input.left,
        y0: input.top,
        x1: input.right,
        y1: input.bottom,
    }
}

/// Convert Stylo and Taffy types into Kurbo types
fn create_css_rect(style: &ComputedValues, layout: &Layout, scale: f64) -> CssBox {
    // Resolve and rescale
    // We have to scale since document pixels are not same same as rendered pixels
    let width: f64 = layout.size.width as f64;
    let height: f64 = layout.size.height as f64;
    let border_box = Rect::new(0.0, 0.0, width * scale, height * scale);
    let border = insets_from_taffy_rect(layout.border.map(|p| p as f64 * scale));
    let padding = insets_from_taffy_rect(layout.padding.map(|p| p as f64 * scale));
    let outline_width = style.get_outline().outline_width.0.to_f64_px() * scale;

    // Resolve the radii to a length. need to downscale since the radii are in document pixels
    let resolve_w = CSSPixelLength::new(width as _);
    let resolve_h = CSSPixelLength::new(height as _);
    let resolve_radii = |radius: &BorderCornerRadius| -> Vec2 {
        Vec2 {
            x: scale * radius.0.width.0.resolve(resolve_w).px() as f64,
            y: scale * radius.0.height.0.resolve(resolve_h).px() as f64,
        }
    };
    let s_border = style.get_border();
    let border_radii = NonUniformRoundedRectRadii {
        top_left: resolve_radii(&s_border.border_top_left_radius),
        top_right: resolve_radii(&s_border.border_top_right_radius),
        bottom_right: resolve_radii(&s_border.border_bottom_right_radius),
        bottom_left: resolve_radii(&s_border.border_bottom_left_radius),
    };

    CssBox::new(border_box, border, padding, outline_width, border_radii)
}

#[cfg(test)]
mod fieldset_tests {
    use super::fieldset_border_cutout;
    use kurbo::Rect;

    #[test]
    fn cutout_snaps_to_device_pixels() {
        // Chromium 153: a legend at 24..81.39 (CSS = device pixels) leaves
        // device pixels 24..=80 of the border unpainted and paints pixel 81.
        let cutout =
            fieldset_border_cutout(Rect::new(24.0, 0.0, 81.39, 18.0), 10.0, 1.0, (0.0, 0.0));
        assert_eq!(cutout, Rect::new(24.0, 0.0, 81.0, 18.0));
        // Snapped in device pixels after the box's offset, halves rounding
        // up (82 + 12.5 snaps to 95; at scale 2, 0.5 + 25 snaps to 26).
        let cutout =
            fieldset_border_cutout(Rect::new(12.5, 0.0, 77.578, 18.0), 10.0, 1.0, (82.0, 36.0));
        assert_eq!(cutout, Rect::new(13.0, 0.0, 78.0, 18.0));
        let cutout =
            fieldset_border_cutout(Rect::new(12.5, 0.0, 77.5, 18.0), 10.0, 2.0, (0.5, 0.0));
        assert_eq!(cutout, Rect::new(25.5, 0.0, 155.5, 36.0));
    }

    #[test]
    fn cutout_spans_a_thicker_border() {
        // A small legend centred on a 10px border: the whole thickness.
        let cutout = fieldset_border_cutout(Rect::new(12.0, 1.0, 40.0, 9.0), 10.0, 1.0, (0.0, 0.0));
        assert_eq!(cutout, Rect::new(12.0, 0.0, 40.0, 10.0));
    }
}
