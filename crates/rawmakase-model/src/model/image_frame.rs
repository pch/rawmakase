//! Image space: positions normalised to the oriented photo before lens correction,
//! Transform, crop and straightening, where masks, spots and red eye corrections are
//! kept, so they stay on the photo whatever those settings do. Sizes are fractions
//! of the long edge.
use crate::camera_data::CameraImage;

/// Image space of one decoded image (or pyramid level) of a photo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageFrame {
    width: u32,
    height: u32,
    /// The camera's default crop, as fractions of the decoded image: left, top, width,
    /// height.
    pub inset: [f32; 4],
    /// Quarter turns of the camera orientation.
    pub turns: u8,
}
impl ImageFrame {
    pub fn new(im: &CameraImage) -> Self {
        Self::with_size(&im.metadata, im.width, im.height)
    }
    /// The frame from metadata alone, as when a Lightroom edit is converted before the
    /// photo is decoded.
    pub fn for_metadata(m: &crate::camera_data::Metadata) -> Self {
        Self::with_size(m, m.width.max(1), m.height.max(1))
    }
    fn with_size(m: &crate::camera_data::Metadata, width: u32, height: u32) -> Self {
        let cw = if m.crop_width > 0 && m.crop_width <= m.width {
            m.crop_width as f32 / m.width as f32
        } else {
            1.
        };
        let ch = if m.crop_height > 0 && m.crop_height <= m.height {
            m.crop_height as f32 / m.height as f32
        } else {
            1.
        };
        let turns = match m.flip {
            3 => 2,
            5 => 3,
            6 => 1,
            _ => 0,
        };
        Self {
            width,
            height,
            inset: [
                if m.crop_left.saturating_add(m.crop_width) <= m.width {
                    m.crop_left as f32 / m.width as f32
                } else {
                    (1. - cw) / 2.
                },
                if m.crop_top.saturating_add(m.crop_height) <= m.height {
                    m.crop_top as f32 / m.height as f32
                } else {
                    (1. - ch) / 2.
                },
                cw,
                ch,
            ],
            turns,
        }
    }
    /// Oriented size in decoded pixels.
    pub fn size(&self) -> [f32; 2] {
        let w = self.width as f32 * self.inset[2];
        let h = self.height as f32 * self.inset[3];
        if self.turns % 2 == 1 { [h, w] } else { [w, h] }
    }
    /// Width over height of the oriented photo.
    pub fn aspect(&self) -> f32 {
        let [w, h] = self.size();
        w / h
    }
    /// Long edge in decoded pixels, the unit of image-space sizes.
    pub fn long_edge(&self) -> f32 {
        let [w, h] = self.size();
        w.max(h)
    }
    /// Image-space position of decoded sample coordinates (pixel `i` centred at `i`).
    pub fn to_image(&self, sx: f32, sy: f32) -> [f32; 2] {
        let x = ((sx + 0.5) / self.width as f32 - self.inset[0]) / self.inset[2];
        let y = ((sy + 0.5) / self.height as f32 - self.inset[1]) / self.inset[3];
        turn((4 - self.turns) % 4, x, y)
    }
    /// Image-space position of a point in the unrotated frame (normalised to the
    /// default crop, before the camera orientation), where Lightroom keeps positions.
    pub fn from_unrotated(&self, p: [f32; 2]) -> [f32; 2] {
        turn((4 - self.turns) % 4, p[0], p[1])
    }
    /// Image-space `[left, top, right, bottom]` of a rectangle in the unrotated frame,
    /// such as the camera's aspect-ratio crop.
    pub fn rect_from_unrotated(&self, [l, t, r, b]: [f32; 4]) -> [f32; 4] {
        let [x0, y0] = self.from_unrotated([l, t]);
        let [x1, y1] = self.from_unrotated([r, b]);
        [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
    }
    /// Decoded sample coordinates of an image-space position.
    pub fn to_source(&self, p: [f32; 2]) -> [f32; 2] {
        let [x, y] = turn(self.turns, p[0], p[1]);
        [
            (self.inset[0] + x * self.inset[2]) * self.width as f32 - 0.5,
            (self.inset[1] + y * self.inset[3]) * self.height as f32 - 0.5,
        ]
    }
}
/// `turns` quarter turns of normalised coordinates, as `Geometry::source` applies them.
pub fn turn(turns: u8, x: f32, y: f32) -> [f32; 2] {
    match turns % 4 {
        1 => [y, 1. - x],
        2 => [1. - x, 1. - y],
        3 => [1. - y, x],
        _ => [x, y],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(flip: i32) -> CameraImage {
        CameraImage {
            width: 300,
            height: 200,
            pixels: vec![[0.2; 3]; 300 * 200],
            metadata: crate::camera_data::Metadata {
                width: 300,
                height: 200,
                crop_left: 10,
                crop_top: 6,
                crop_width: 280,
                crop_height: 190,
                flip,
                wb: [1.; 3],
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            recovered: Default::default(),
        }
    }
    #[test]
    fn image_frame_round_trips_every_orientation() {
        for flip in [0, 3, 5, 6] {
            let f = ImageFrame::new(&image(flip));
            for p in [[0.1, 0.2], [0.5, 0.5], [0.93, 0.07]] {
                let [x, y] = f.to_source(p);
                let q = f.to_image(x, y);
                assert!((p[0] - q[0]).abs() < 1e-5 && (p[1] - q[1]).abs() < 1e-5);
            }
            let size = f.size();
            let turned = flip == 5 || flip == 6;
            assert_eq!(size, if turned { [190., 280.] } else { [280., 190.] });
        }
    }
    /// The camera's aspect-ratio crop is a crop of the unrotated frame; a photo held
    /// upright starts from the same area of the sensor, turned with it.
    #[test]
    fn camera_crop_turns_with_the_photo() {
        let square = [1. / 6., 0., 5. / 6., 1.];
        for (flip, want) in [
            (0, square),
            (3, square),
            (6, [0., 1. / 6., 1., 5. / 6.]),
            (5, [0., 1. / 6., 1., 5. / 6.]),
        ] {
            let mut m = image(flip).metadata;
            m.camera_crop = Some([1. / 6., 0., 5. / 6., 1.]);
            let crop = crate::model::recipe::Recipe::for_metadata(&m).crop;
            for (got, want) in crop.iter().zip(want) {
                assert!((got - want).abs() < 1e-6, "flip {flip}: {crop:?}");
            }
        }
        // An off-centre crop keeps its place on the sensor.
        let mut m = image(6).metadata;
        m.camera_crop = Some([0., 0., 0.5, 1.]);
        let [x, y] = ImageFrame::for_metadata(&m).from_unrotated([0.25, 0.5]);
        let crop = crate::model::recipe::Recipe::for_metadata(&m).crop;
        assert!(
            crop[0] <= x && x <= crop[2] && crop[1] <= y && y <= crop[3],
            "{crop:?}"
        );
    }
}
