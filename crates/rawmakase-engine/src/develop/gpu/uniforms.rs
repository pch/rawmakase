//! The uniform parameter blocks of `present.wgsl` and `finish.wgsl`, declared once
//! in Rust with the shader's field names. A test parses each shader with naga and
//! checks that its `Params` struct has the same fields at the same offsets, so a
//! field added, moved or retyped on one side fails CI without a GPU.

/// A `#[repr(C)]` plain-old-data struct whose field names, types and byte offsets
/// are listed in `FIELDS`, for comparison with the shader's declaration.
macro_rules! uniform {
    ($(#[$meta:meta])* $name:ident { $($(#[$field_meta:meta])* $field:ident: $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[repr(C)]
        #[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
        pub(crate) struct $name {
            $($(#[$field_meta])* pub(crate) $field: $ty),*
        }
        impl $name {
            /// Every field's name, type and byte offset, in declaration order.
            #[cfg(test)]
            pub(crate) const FIELDS: &[(&str, &str, usize)] = &[$((
                stringify!($field),
                stringify!($ty),
                std::mem::offset_of!($name, $field),
            )),*];
        }
    };
}

uniform! {
    /// `Params` in `finish.wgsl`: sharpening and Lanczos resampling of a preview.
    FinishParams {
        width: u32,
        height: u32,
        out_width: u32,
        out_height: u32,
        radius: u32,
        x_stride: u32,
        y_stride: u32,
        y_offset: u32,
        amount: f32,
        threshold: f32,
        halo: f32,
        dark: f32,
    }
}

uniform! {
    /// `Params` in `present.wgsl`: sharpening, effects, the clipping overlay and
    /// the monitor profile of the preview drawn on screen.
    PresentParams {
        /// Developed buffer size, and the rectangle of it that is shown.
        width: u32,
        height: u32,
        crop_x: u32,
        crop_y: u32,
        crop_w: u32,
        crop_h: u32,
        radius: u32,
        sharpen: u32,
        amount: f32,
        threshold: f32,
        clipping: u32,
        lut_size: u32,
        /// Output pixel of the buffer's first pixel, and the whole output's size.
        origin_x: u32,
        origin_y: u32,
        full_w: u32,
        full_h: u32,
        scale: f32,
        /// `effects::GrainField`.
        grain: f32,
        grain_cell: f32,
        grain_coarse: f32,
        grain_seed: u32,
        /// `effects::PostCropVignette`; a zero amount has no vignette.
        vignette: f32,
        vignette_style: u32,
        vignette_highlights: f32,
        vignette_scale_x: f32,
        vignette_scale_y: f32,
        vignette_power: f32,
        vignette_midpoint: f32,
        vignette_feather: f32,
        effects: u32,
        count: u32,
        halo: f32,
        dark: f32,
        grain_fine: f32,
        pad3: u32,
        pad4: u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::naga::{Scalar, ScalarKind, TypeInner, front::wgsl};

    /// Each member of the struct `Params` in `source`, by name, Rust type name and
    /// byte offset, and the struct's size.
    fn shader_params(source: &str) -> (Vec<(String, String, usize)>, usize) {
        let module = wgsl::parse_str(source).expect("valid WGSL");
        let (_, params) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("Params"))
            .expect("a Params struct");
        let TypeInner::Struct { members, span } = &params.inner else {
            panic!("Params is a struct");
        };
        let rust_type = |ty| match module.types[ty].inner {
            TypeInner::Scalar(Scalar {
                kind: ScalarKind::Uint,
                width: 4,
            }) => "u32",
            TypeInner::Scalar(Scalar {
                kind: ScalarKind::Float,
                width: 4,
            }) => "f32",
            ref other => panic!("no Rust type for {other:?}"),
        };
        let members = members
            .iter()
            .map(|m| {
                let name = m.name.clone().unwrap_or_default();
                (name, rust_type(m.ty).to_string(), m.offset as usize)
            })
            .collect();
        (members, *span as usize)
    }

    fn rust_fields(fields: &[(&str, &str, usize)]) -> Vec<(String, String, usize)> {
        fields
            .iter()
            .map(|(name, ty, offset)| ((*name).into(), (*ty).into(), *offset))
            .collect()
    }

    #[test]
    fn present_params_match_the_shader() {
        let source =
            crate::develop::effects::PostCropVignette::wgsl_tone() + include_str!("present.wgsl");
        let (members, size) = shader_params(&source);
        assert_eq!(members, rust_fields(PresentParams::FIELDS));
        assert_eq!(size, std::mem::size_of::<PresentParams>());
    }

    #[test]
    fn finish_params_match_the_shader() {
        let (members, size) = shader_params(include_str!("finish.wgsl"));
        assert_eq!(members, rust_fields(FinishParams::FIELDS));
        assert_eq!(size, std::mem::size_of::<FinishParams>());
    }
}
