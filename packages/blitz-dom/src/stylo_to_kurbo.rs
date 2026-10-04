use euclid::default::Rect;
use kurbo::{Affine, Vec2};
use style::{
    properties::generated::style_structs::Box as BoxStyleStruct,
    values::{
        computed::{CSSPixelLength, Rotate},
        generics::transform::{Scale, Translate},
    },
};

// 6. Current Transformation Matrix
//
// The transformation matrix is computed from the transform, transform-origin, translate, rotate, scale, and offset properties as follows:
//
//   - Start with the identity matrix.
//   - Translate by the computed X, Y, and Z values of transform-origin.
//   - Translate by the computed X, Y, and Z values of translate.
//   - Rotate by the computed <angle> about the specified axis of rotate.
//   - Scale by the computed X, Y, and Z values of scale.
//   - Translate and rotate by the transform specified by offset.
//   - Multiply by each of the transform functions in transform from left to right.
//   - Translate by the negated computed X, Y and Z values of transform-origin.
//
// <https://drafts.csswg.org/css-transforms-2/#ctm>
pub fn resolve_2d_transform(
    box_styles: &BoxStyleStruct,
    reference_box: Rect<CSSPixelLength>,
) -> Option<Affine> {
    let translate = match &box_styles.translate {
        Translate::None => None,
        Translate::Translate(x, y, _z) => Some(Vec2 {
            x: x.resolve(reference_box.width()).px() as f64,
            y: y.resolve(reference_box.height()).px() as f64,
        }),
    };

    let rotate = match &box_styles.rotate {
        Rotate::None => None,
        Rotate::Rotate(angle) => Some(angle.radians64()),
        // A rotation about the z axis stays in the plane; the sign of the
        // axis component decides the direction, its magnitude cancels out.
        Rotate::Rotate3D(x, y, z, angle) if *x == 0.0 && *y == 0.0 && *z != 0.0 => {
            Some(angle.radians64() * z.signum() as f64)
        }
        // TODO: support 3D transforms
        Rotate::Rotate3D(_, _, _, _) => None,
    };

    let scale_transform = match &box_styles.scale {
        Scale::None => None,
        Scale::Scale(x, y, _z) => Some(Vec2 {
            x: *x as f64,
            y: *y as f64,
        }),
    };

    let transform = if box_styles.transform.0.is_empty() {
        None
    } else {
        box_styles
            .transform
            .to_transform_3d_matrix(Some(&reference_box))
            .ok()
            // A flat element (`transform-style: flat`, outside a 3d rendering
            // context) shows its transform flattened: its own points have
            // z = 0, so without perspective terms (m14 = m24 = 0) the 4x4
            // matrix maps them as the affine part divided by m44, whatever
            // its z row and column hold (`scale3d(s, s, s)`, `translate3d`,
            // `rotateY(50deg)` painted narrower). A perspective term needs a
            // projective map, which an Affine cannot hold.
            // See: https://drafts.csswg.org/css-transforms-2/#3d-transform-rendering
            .filter(|(t, _has_3d)| t.m14 == 0.0 && t.m24 == 0.0 && t.m44 != 0.0)
            .map(|(t, _)| {
                // See: https://drafts.csswg.org/css-transforms-2/#two-dimensional-subset
                // And https://docs.rs/kurbo/latest/kurbo/struct.Affine.html#method.new
                let w = t.m44 as f64;
                Affine::new([t.m11, t.m12, t.m21, t.m22, t.m41, t.m42].map(|v| v as f64 / w))
            })
    };

    // TODO: support the "offset" property
    // <https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/Properties/offset>

    if translate.is_none() && rotate.is_none() && scale_transform.is_none() && transform.is_none() {
        return None;
    }

    // Apply the transform origin by:
    //   - Translating by the origin offset
    //   - Applying our transform
    //   - Translating by the inverse of the origin offset
    let transform_origin = &box_styles.transform_origin;
    let origin_translation = Affine::translate(Vec2 {
        x: transform_origin
            .horizontal
            .resolve(reference_box.width())
            .px() as f64,
        y: transform_origin
            .vertical
            .resolve(reference_box.height())
            .px() as f64,
    });

    let mut resolved = Affine::IDENTITY;

    if let Some(translation) = translate {
        resolved *= Affine::translate(translation)
    }

    if let Some(rotation) = rotate {
        resolved *= Affine::rotate(rotation)
    }

    if let Some(scale_transform) = scale_transform {
        resolved *= Affine::scale_non_uniform(scale_transform.x, scale_transform.y)
    }

    if let Some(transform) = transform {
        resolved *= transform;
    }

    resolved = origin_translation * resolved * origin_translation.inverse();

    if resolved != Affine::IDENTITY {
        Some(resolved)
    } else {
        None
    }
}
