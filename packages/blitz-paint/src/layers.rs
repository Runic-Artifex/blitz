use anyrender::{Filter, PaintScene};
use kurbo::{Affine, BezPath, Shape};
use peniko::{Fill, Mix};
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};

const LAYER_LIMIT: u32 = 1024;

/// A clip layer that does not isolate its content in CSS (an `overflow`
/// clip, `clip`, or such a clip re-applied around a hoisted child), open on
/// the scene.
#[derive(Clone)]
pub(crate) struct BackdropClip {
    transform: Affine,
    shape: BezPath,
}

#[derive(Default)]
pub(crate) struct LayerManager {
    layers_used: Cell<u32>,
    layer_depth: Cell<u32>,
    layers_wanted: Cell<u32>,

    #[allow(unused)] // Only used for debugging. Enabled as required.
    layer_depth_used: Cell<u32>,

    /// The clip layers that do not isolate, per open isolated group (the
    /// scene itself, then each layer that isolates the content drawn into
    /// it), innermost group last. In CSS an element with `mix-blend-mode`
    /// blends with everything its isolated group has painted behind it,
    /// also outside such clips; the renderers blend a layer with the layer
    /// it is drawn into (vello's clip layers are also partly isolating:
    /// vello#1198). A blended element therefore closes the clips of its
    /// group before its blend layer and draws them again inside it
    /// ([`Self::suspend_backdrop_clips`]).
    groups: RefCell<Vec<Vec<BackdropClip>>>,
}

impl LayerManager {
    /// Starts an isolated group: a layer that isolates its content has just
    /// been pushed.
    pub(crate) fn begin_group(&self) {
        self.groups.borrow_mut().push(Vec::new());
    }

    /// Ends the isolated group begun last, before its layer is popped.
    pub(crate) fn end_group(&self) {
        let group = self.groups.borrow_mut().pop();
        debug_assert!(group.is_none_or(|clips| clips.is_empty()));
    }

    /// Pushes a clip layer that does not isolate its content (when
    /// `condition` holds and the layer limit allows), recording it in the
    /// current group. Returns whether it was pushed.
    pub(crate) fn maybe_push_backdrop_clip(
        &self,
        scene: &mut impl PaintScene,
        condition: bool,
        transform: Affine,
        shape: &impl Shape,
    ) -> bool {
        let pushed = self.maybe_push_layer(scene, condition, 1.0, transform, shape, None, None);
        if pushed {
            self.record_backdrop_clip(transform, shape);
        }
        pushed
    }

    /// Pops a clip layer pushed by [`Self::maybe_push_backdrop_clip`].
    pub(crate) fn maybe_pop_backdrop_clip(&self, scene: &mut impl PaintScene, pushed: bool) {
        if pushed {
            self.forget_backdrop_clip();
            self.maybe_pop_layer(scene, true);
        }
    }

    /// Records a non-isolating clip layer just pushed on the scene.
    pub(crate) fn record_backdrop_clip(&self, transform: Affine, shape: &impl Shape) {
        let mut groups = self.groups.borrow_mut();
        if groups.is_empty() {
            groups.push(Vec::new());
        }
        groups.last_mut().unwrap().push(BackdropClip {
            transform,
            shape: shape.to_path(0.1),
        });
    }

    /// Forgets the non-isolating clip layer recorded last, about to be popped.
    pub(crate) fn forget_backdrop_clip(&self) {
        let clip = self
            .groups
            .borrow_mut()
            .last_mut()
            .and_then(|clips| clips.pop());
        debug_assert!(clip.is_some());
    }

    /// Pops the current group's non-isolating clip layers from the scene, so
    /// that a layer pushed next is drawn into the group's own layer, and
    /// returns them innermost last.
    pub(crate) fn suspend_backdrop_clips(&self, scene: &mut impl PaintScene) -> Vec<BackdropClip> {
        let clips = self
            .groups
            .borrow_mut()
            .last_mut()
            .map(std::mem::take)
            .unwrap_or_default();
        for _ in &clips {
            scene.pop_layer();
        }
        clips
    }

    /// Pushes clip layers returned by [`Self::suspend_backdrop_clips`] again,
    /// recording them in the current group.
    pub(crate) fn resume_backdrop_clips(
        &self,
        scene: &mut impl PaintScene,
        clips: &[BackdropClip],
    ) {
        for clip in clips {
            scene.push_clip_layer(Fill::NonZero, clip.transform, &clip.shape);
            self.record_backdrop_clip(clip.transform, &clip.shape);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn maybe_with_layer<S: PaintScene, F: FnOnce(&mut S)>(
        &self,
        scene: &mut S,
        condition: bool,
        opacity: f32,
        transform: Affine,
        shape: &impl Shape,
        filter: Option<Arc<Filter>>,
        backdrop_filter: Option<Arc<Filter>>,
        paint_layer: F,
    ) {
        let layer_used = self.maybe_push_layer(
            scene,
            condition,
            opacity,
            transform,
            shape,
            filter,
            backdrop_filter,
        );
        paint_layer(scene);
        self.maybe_pop_layer(scene, layer_used);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn maybe_push_layer(
        &self,
        scene: &mut impl PaintScene,
        condition: bool,
        opacity: f32,
        transform: Affine,
        shape: &impl Shape,
        filter: Option<Arc<Filter>>,
        backdrop_filter: Option<Arc<Filter>>,
    ) -> bool {
        if !condition {
            return false;
        }
        self.layers_wanted.update(|x| x + 1);

        // Check if clips are above limit
        let layers_available = self.layers_used.get() <= LAYER_LIMIT;
        if !layers_available {
            return false;
        }

        // Actually push the layer
        if opacity == 1.0 && filter.is_none() && backdrop_filter.is_none() {
            scene.push_clip_layer(Fill::NonZero, transform, shape);
        } else {
            scene.push_layer(
                Fill::NonZero,
                Mix::Normal,
                opacity,
                transform,
                shape,
                filter,
                backdrop_filter,
            );
        };

        // Update accounting
        self.layers_used.update(|x| x + 1);
        self.layer_depth.update(|x| x + 1);
        self.layer_depth.update(|x| x.max(self.layer_depth.get()));

        true
    }

    pub(crate) fn maybe_pop_layer(&self, scene: &mut impl PaintScene, condition: bool) {
        if condition {
            scene.pop_layer();
            self.layer_depth.update(|x| x - 1);
        }
    }
}
