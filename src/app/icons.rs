//! The interface's icons: Lucide outlines (fastframe-icons), drawn in white
//! and tinted where they are painted. RAWmakase's own copies live in
//! `assets/icons`; the rest come from fastframe's shared set.
use eframe::egui::{self, Color32, Rect};

fastframe_icons::icons! {
    /// Every icon the interface draws.
    pub(super) enum Icon {
        prefix: "rawmakase-icon-",
        directory: "../../assets/icons/",
        Undo => "undo-2",
        Redo => "redo-2",
        BeforeAfter => "columns-2",
        Export => "download",
        Reset => "rotate-ccw",
        Eyedropper => "pipette",
        Crop => "crop",
        Flag => "flag",
        Rejected => "flag-off",
        Folder => "folder",
        FolderPlus => "folder-plus",
        PanelLeftOpen => "panel-left-open",
        PanelRight => "panel-right",
        PanelRightOpen => "panel-right-open",
        PanelBottom => "panel-bottom",
        PanelBottomOpen => "panel-bottom-open",
        Collection => "images",
        CollectionSet => "box",
        Keyboard => "keyboard",
        Bug => "bug",
        GridView => "layout-grid",
        LoupeView => "square",
        SurveyView => "layout-dashboard",
        Presets => "layers",
        Bandage => "bandage",
        MaskTool => "circle-dot-dashed",
        History => "history",
        Edit => "sliders-horizontal",
        RotateLeft => "rotate-ccw-square",
        RotateRight => "rotate-cw-square",
        FlipHorizontal => "flip-horizontal-2",
        FlipVertical => "flip-vertical-2",
        SwapOrientation => "arrow-left-right",
        Settings => lucide "settings",
        Catalog => "library-big",
        Performance => "gauge",
        Automation => "zap",
        Monitor => lucide "monitor",
        Close => lucide "x",
        Check => lucide "check",
        Add => lucide "plus",
        More => lucide "ellipsis",
        ChevronDown => lucide "chevron-down",
        ChevronRight => lucide "chevron-right",
        PanelLeft => lucide "panel-left",
        Eye => lucide "eye",
        Search => lucide "search",
        EyeOff => lucide "eye-off",
    }
}

/// Registers the icons with egui, after its SVG loader.
pub(super) fn install(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
    fastframe_icons::install::<Icon>(ctx);
}

/// Paints `icon` into `rect`, tinted. Nothing is drawn until the SVG is
/// rasterized, which happens on first use at each size.
pub(super) fn paint(painter: &egui::Painter, icon: Icon, rect: Rect, tint: Color32) {
    let ctx = painter.ctx();
    let ppp = ctx.pixels_per_point();
    let rect = pixel_aligned(rect, ppp);
    let hint = egui::SizeHint::Height((rect.height() * ppp).round() as u32);
    if let Ok(egui::load::TexturePoll::Ready { texture }) =
        ctx.try_load_texture(icon.uri(), egui::TextureOptions::LINEAR, hint)
    {
        let uv = Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.));
        painter.image(texture.id, rect, uv, tint);
    }
}

/// Paints `icon` `size` points square, centred on `center`.
pub(super) fn paint_at(
    painter: &egui::Painter,
    icon: Icon,
    center: egui::Pos2,
    size: f32,
    tint: Color32,
) {
    paint(
        painter,
        icon,
        Rect::from_center_size(center, egui::Vec2::splat(size)),
        tint,
    );
}

/// `rect` moved and sized to whole physical pixels, about the same centre.
/// The icon is rasterized at exactly that many pixels; drawn off the pixel
/// grid, linear filtering would blend each of them with its neighbour and
/// blur the outline.
fn pixel_aligned(rect: Rect, pixels_per_point: f32) -> Rect {
    let side = (rect.size() * pixels_per_point)
        .round()
        .max(egui::Vec2::splat(1.));
    let min = (rect.center().to_vec2() * pixels_per_point - side / 2.).round();
    Rect::from_min_size(min.to_pos2(), side) / pixels_per_point
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_pixel_grid(rect: Rect, pixels_per_point: f32) -> bool {
        let physical =
            [rect.min.x, rect.min.y, rect.max.x, rect.max.y].map(|v| v * pixels_per_point);
        physical.iter().all(|v| (v - v.round()).abs() < 1e-3)
    }

    #[test]
    fn an_odd_sized_icon_on_a_whole_point_lands_on_whole_pixels() {
        // A 15 pt icon centred on a button: its edges fall on half pixels.
        let rect = Rect::from_center_size(egui::pos2(100., 40.), egui::Vec2::splat(15.));
        assert!(!on_pixel_grid(rect, 1.));
        let aligned = pixel_aligned(rect, 1.);
        assert!(on_pixel_grid(aligned, 1.));
        assert_eq!(aligned.size(), egui::Vec2::splat(15.));
        let shift = aligned.center() - rect.center();
        assert!(shift.x.abs() <= 0.5 && shift.y.abs() <= 0.5);
    }

    #[test]
    fn an_even_sized_icon_on_a_half_point_lands_on_whole_pixels() {
        let rect = Rect::from_center_size(egui::pos2(100.5, 40.5), egui::Vec2::splat(16.));
        let aligned = pixel_aligned(rect, 1.);
        assert!(on_pixel_grid(aligned, 1.));
        assert_eq!(aligned.size(), egui::Vec2::splat(16.));
    }

    #[test]
    fn icons_land_on_whole_pixels_at_fractional_scales() {
        for ppp in [1.25, 1.5, 1.75, 2.] {
            for size in [11., 12., 13., 15., 16.] {
                let rect = Rect::from_center_size(egui::pos2(73.3, 18.7), egui::Vec2::splat(size));
                let aligned = pixel_aligned(rect, ppp);
                assert!(on_pixel_grid(aligned, ppp), "{size} pt at {ppp}x");
                let pixels = aligned.height() * ppp;
                assert!(
                    (pixels - (size * ppp).round()).abs() < 1e-3,
                    "{size} pt at {ppp}x"
                );
            }
        }
    }
}
