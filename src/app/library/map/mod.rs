//! The map under the Metadata panel's GPS row: where the photos shown were
//! taken, on tiles from the server named in Preferences › General. It is off
//! until the user turns it on there, as each tile asked for tells the server
//! roughly where the photos are. Fetching is in [`fetch`], the map's
//! geometry in [`tiles`].
mod fetch;
pub(in crate::app) mod tiles;

use crate::app::ui::design::{Tone, radius, size, space, text};
use crate::app::ui::theme;
use eframe::egui::{self, Color32, Sense, Vec2};
use fetch::{Fetcher, Net};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tiles::{MAX_ZOOM, TILE, Template, TileId, World};

/// Tile textures kept; a map shows at most a few at once.
const KEPT: usize = 96;
/// A tile that could not be fetched is asked for again after this.
const RETRY: Duration = Duration::from_secs(30);
/// One photo's place is shown this close.
const PLACE_ZOOM: u8 = 15;
/// Room kept between the outermost places and the map's edge.
const MARGIN: f64 = 24.;
/// The pins' colour: map tiles are light whatever the theme, so it is fixed,
/// the red map apps mark a place with.
const PIN: Color32 = Color32::from_rgb(226, 62, 51);
/// How far up a tile not fetched yet is drawn from: a coarser one scaled up.
const FALLBACK_LEVELS: u8 = 4;

/// The server the map's tiles come from and the credit they carry. None
/// while the map is off.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct Source {
    pub template: Arc<Template>,
    pub credit: String,
}

/// Preferences › General's Map: whether it is on, the tile server's address
/// and its credit, as the user typed them. Saved in the session.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct Settings {
    pub on: bool,
    pub tiles: String,
    pub credit: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            on: false,
            tiles: tiles::OPENSTREETMAP.into(),
            credit: tiles::OPENSTREETMAP_CREDIT.into(),
        }
    }
}
impl Settings {
    /// From the session's values: an address or credit left empty is
    /// OpenStreetMap's.
    pub(in crate::app) fn of(on: bool, tiles: Option<&str>, credit: Option<&str>) -> Self {
        let d = Self::default();
        let given = |v: Option<&str>| {
            v.map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        Self {
            on,
            tiles: given(tiles).unwrap_or(d.tiles),
            credit: given(credit).unwrap_or(d.credit),
        }
    }
    /// The session's values: None where they are OpenStreetMap's.
    pub(in crate::app) fn to_session(&self) -> (bool, Option<String>, Option<String>) {
        let d = Self::default();
        let changed =
            |v: &str, d: &str| (v.trim() != d && !v.trim().is_empty()).then(|| v.trim().to_owned());
        (
            self.on,
            changed(&self.tiles, &d.tiles),
            changed(&self.credit, &d.credit),
        )
    }
    /// Why the address can't be used, if it can't.
    pub(in crate::app) fn problem(&self) -> Option<&'static str> {
        Template::parse(&self.tiles).err()
    }
    /// The map's source, when it is on and its address can be used.
    pub(in crate::app) fn source(&self) -> Option<Source> {
        let template = Template::parse(&self.tiles).ok().filter(|_| self.on)?;
        Some(Source {
            template: Arc::new(template),
            credit: self.credit.trim().to_owned(),
        })
    }
}

/// Where the map looks, until the photos shown change.
struct View {
    places: Vec<(u64, u64)>,
    centre: World,
    zoom: u8,
    /// Pinching not yet a whole zoom step, in steps.
    pinch: f32,
}

pub(in crate::app::library) struct Map {
    source: Option<Source>,
    fetcher: Option<Fetcher>,
    root: PathBuf,
    net: Net,
    textures: HashMap<TileId, egui::TextureHandle>,
    order: VecDeque<TileId>,
    failed: HashMap<TileId, (Instant, String)>,
    view: Option<View>,
}
impl Map {
    pub(in crate::app::library) fn new() -> Self {
        Self::at(
            crate::decode_cache::cache_dir().join("map-tiles"),
            Net::default(),
        )
    }
    fn at(root: PathBuf, net: Net) -> Self {
        Self {
            source: None,
            fetcher: None,
            root,
            net,
            textures: HashMap::new(),
            order: VecDeque::new(),
            failed: HashMap::new(),
            view: None,
        }
    }
    /// Tiles kept in `root`, fetched from a plain HTTP server, for tests.
    #[cfg(test)]
    pub(in crate::app) fn for_tests(root: PathBuf) -> Self {
        Self::at(root, Net { https_only: false })
    }
    /// Shows the map from `source` from now on, or none; the workspace sets it
    /// every frame. Tiles of another server are dropped, and arrive no more.
    pub(in crate::app::library) fn set_source(&mut self, source: Option<Source>) {
        if self.source == source {
            return;
        }
        if self.source.as_ref().map(|s| &s.template) != source.as_ref().map(|s| &s.template) {
            self.textures.clear();
            self.order.clear();
            self.failed.clear();
        }
        self.source = source;
    }
    pub(in crate::app::library) fn is_on(&self) -> bool {
        self.source.is_some()
    }
    /// Ends the tile workers at exit, without waiting for them here.
    pub(in crate::app::library) fn close(&mut self) -> Vec<crate::app::task::Stopping> {
        self.fetcher
            .as_mut()
            .map(Fetcher::close)
            .unwrap_or_default()
    }
    fn receive(&mut self, ctx: &egui::Context) {
        let (Some(fetcher), Some(source)) = (&self.fetcher, &self.source) else {
            return;
        };
        for done in fetcher.done() {
            if done.template != source.template {
                continue;
            }
            match done.image {
                Ok(image) => {
                    let name = format!("map-{}-{}-{}", done.tile.z, done.tile.x, done.tile.y);
                    let texture = ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
                    self.failed.remove(&done.tile);
                    if self.textures.insert(done.tile, texture).is_none() {
                        self.order.push_back(done.tile);
                    }
                    while self.order.len() > KEPT {
                        if let Some(old) = self.order.pop_front() {
                            self.textures.remove(&old);
                        }
                    }
                }
                Err(e) => {
                    self.failed.insert(done.tile, (Instant::now(), e));
                }
            }
        }
    }

    /// Draws the map of `places` (latitude and longitude, in degrees) across
    /// the panel, with a row below it to open the place in the browser. Draws
    /// nothing while the map is off or there is no place.
    pub(in crate::app::library) fn show(&mut self, ui: &mut egui::Ui, places: &[(f64, f64)]) {
        let Some(source) = self.source.clone() else {
            return;
        };
        if places.is_empty() {
            return;
        }
        self.receive(ui.ctx());
        let palette = theme::palette(ui.ctx());
        ui.add_space(space::XS);
        let width = ui.available_width().max(1.);
        let height = (width * 0.62).clamp(140., 240.).round();
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
        // The panel scrolls, and egui runs what it clips: a map out of sight
        // keeps its place in the layout but asks for no tile.
        if !ui.is_rect_visible(rect) {
            ui.add_space(space::XS);
            ui.allocate_exact_size(Vec2::new(width, size::ROW), Sense::hover());
            return;
        }
        let world: Vec<World> = places
            .iter()
            .map(|(lat, lon)| World::of(*lat, *lon))
            .collect();
        let key: Vec<(u64, u64)> = places
            .iter()
            .map(|(a, b)| (a.to_bits(), b.to_bits()))
            .collect();
        if self.view.as_ref().is_none_or(|v| v.places != key) {
            let zoom = tiles::fit(
                &world,
                f64::from(width),
                f64::from(height),
                MARGIN,
                PLACE_ZOOM,
            );
            self.view = Some(View {
                places: key,
                centre: tiles::centre(&world).unwrap_or(World { x: 0.5, y: 0.5 }),
                zoom,
                pinch: 0.,
            });
        }
        let view = self.view.as_mut().expect("set above");
        steer(view, &response, ui, rect);
        let (centre, zoom) = (view.centre, view.zoom);

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, radius::SMALL, palette.tone(Tone::FIELD));
        let shown = tiles::visible(centre, zoom, f64::from(width), f64::from(height));
        let mut missing = Vec::new();
        let mut failures = 0;
        for (tile, x, y) in &shown {
            let at = egui::Rect::from_min_size(
                rect.min + Vec2::new(*x as f32, *y as f32),
                Vec2::splat(TILE as f32),
            );
            if let Some(texture) = self.textures.get(tile) {
                painter.image(texture.id(), at, full_uv(), Color32::WHITE);
                continue;
            }
            if let Some((texture, uv)) = self.coarser(*tile) {
                painter.image(texture, at, uv, Color32::WHITE);
            }
            match self.failed.get(tile) {
                Some((at, _)) if at.elapsed() < RETRY => failures += 1,
                _ => missing.push(*tile),
            }
        }
        // A map wider than the world at zoom 0 or 1 shows a tile more than once.
        let mut seen = std::collections::HashSet::new();
        missing.retain(|t| seen.insert(*t));
        if failures > 0 {
            // A failed tile is asked for again once RETRY has passed, even
            // while nothing else repaints.
            ui.ctx().request_repaint_after(RETRY);
        }
        if !missing.is_empty() {
            let fetcher = self.fetcher.get_or_insert_with(|| {
                Fetcher::new(self.root.clone(), self.net.clone(), ui.ctx().clone())
            });
            fetcher.want(&source.template, missing);
        }
        // Each place as a pin, its point where the photo was taken.
        let span = tiles::span(zoom);
        for place in &world {
            let mut dx = (place.x - centre.x) * span;
            // The nearest copy of a place across the date line.
            dx -= (dx / span).round() * span;
            let at = rect.center() + Vec2::new(dx as f32, ((place.y - centre.y) * span) as f32);
            if rect.expand(8.).contains(at) {
                painter.circle_filled(at + Vec2::new(0., 1.), 7., Color32::from_black_alpha(70));
                painter.circle(at, 6., PIN, egui::Stroke::new(2., Color32::WHITE));
            }
        }
        if failures == shown.len() && failures > 0 {
            let error = self
                .failed
                .values()
                .next()
                .map(|e| e.1.clone())
                .unwrap_or_default();
            // Below the pin, which stays where the place is.
            painter.text(
                rect.center() + Vec2::new(0., space::XXL),
                egui::Align2::CENTER_CENTER,
                "Map unavailable",
                egui::FontId::proportional(text::SMALL),
                palette.tone(Tone::MUTED),
            );
            response
                .clone()
                .on_hover_text(format!("The map could not be loaded: {error}"));
        } else {
            response
                .clone()
                .on_hover_text("Drag to move, double-click or pinch to zoom");
        }
        painter.rect_stroke(
            rect,
            radius::SMALL,
            egui::Stroke::new(1., palette.tone(Tone::OUTLINE)),
            egui::StrokeKind::Inside,
        );
        self.zoom_buttons(ui, rect);
        credit(ui, rect, &source.credit);
        ui.add_space(space::XS);
        let (row, _) = ui.allocate_exact_size(Vec2::new(width, size::ROW), Sense::hover());
        // Its text ends where the GPS row's Clear does.
        let label = ui
            .painter()
            .layout_no_wrap(
                "Open in Browser".into(),
                egui::FontId::proportional(text::SMALL),
                Color32::WHITE,
            )
            .size()
            .x;
        let open = ui
            .put(
                egui::Rect::from_min_max(
                    egui::pos2(row.right() - label - 2. * space::S, row.top()),
                    row.max,
                ),
                egui::Button::new(egui::RichText::new("Open in Browser").size(text::SMALL))
                    .frame(false),
            )
            .on_hover_text("Show this place on openstreetmap.org");
        if open.clicked() {
            let (lat, lon) = self.view.as_ref().map_or((0., 0.), |v| v.centre.degrees());
            let url = browser_url(places, zoom, lat, lon);
            if let Err(e) = crate::platform::web::open(&url) {
                eprintln!("Could not open the map: {e}");
            }
        }
    }
    /// The nearest coarser tile kept, and the part of it `tile` covers.
    fn coarser(&self, tile: TileId) -> Option<(egui::TextureId, egui::Rect)> {
        (1..=FALLBACK_LEVELS.min(tile.z)).find_map(|up| {
            let parent = TileId {
                z: tile.z - up,
                x: tile.x >> up,
                y: tile.y >> up,
            };
            let texture = self.textures.get(&parent)?;
            let n = (1u32 << up) as f32;
            let (u, v) = (
                (tile.x % (1 << up)) as f32 / n,
                (tile.y % (1 << up)) as f32 / n,
            );
            Some((
                texture.id(),
                egui::Rect::from_min_size(egui::pos2(u, v), Vec2::splat(1. / n)),
            ))
        })
    }
    fn zoom_buttons(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let side = size::CONTROL;
        let right = rect.right() - space::S;
        let top = rect.top() + space::S;
        for (i, (label, step, hint)) in [("+", 1i8, "Zoom In"), ("−", -1, "Zoom Out")]
            .into_iter()
            .enumerate()
        {
            let at = egui::Rect::from_min_size(
                egui::pos2(right - side, top + i as f32 * (side + space::XXS)),
                Vec2::splat(side),
            );
            let can = if step > 0 {
                view.zoom < MAX_ZOOM
            } else {
                view.zoom > 0
            };
            if overlay_button(ui, at, label, hint, can) {
                view.zoom = view.zoom.saturating_add_signed(step).min(MAX_ZOOM);
            }
        }
    }
}

/// A button drawn over the map. Not placed by the layout (`ui.put` would move
/// the panel's cursor back up into the map), but a widget of its own, so it
/// takes the keyboard focus and is named to screen readers.
fn overlay_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    label: &str,
    hint: &str,
    enabled: bool,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui
        .interact(rect, ui.id().with(("map-button", hint)), sense)
        .on_hover_text(hint);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, hint));
    let fill = if enabled && response.hovered() {
        Tone::RAISED
    } else {
        Tone::CARD
    };
    let painter = ui.painter();
    painter.rect_filled(rect, radius::CONTROL, palette.tone(fill));
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            radius::CONTROL,
            egui::Stroke::new(1.5, palette.accent()),
            egui::StrokeKind::Outside,
        );
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(text::BODY),
        palette.tone(if enabled { Tone::TEXT } else { Tone::MUTED }),
    );
    enabled && response.clicked()
}

fn full_uv() -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.))
}

/// Moves the view by dragging, and zooms by double-clicking (towards the point
/// clicked) or pinching.
fn steer(view: &mut View, response: &egui::Response, ui: &egui::Ui, rect: egui::Rect) {
    let span = tiles::span(view.zoom);
    if response.dragged() {
        let d = response.drag_delta();
        view.centre.x -= f64::from(d.x) / span;
        view.centre.y -= f64::from(d.y) / span;
    }
    let mut step = 0i8;
    let mut towards = None;
    if response.double_clicked() {
        step = 1;
        towards = response.interact_pointer_pos();
    }
    if response.hovered() {
        let pinch = ui.input(|i| i.zoom_delta());
        if pinch != 1. {
            view.pinch += pinch.log2();
            towards = ui.input(|i| i.pointer.hover_pos());
            if view.pinch.abs() >= 1. {
                step = view.pinch.signum() as i8;
                view.pinch = 0.;
            }
        }
    }
    let zoom = view.zoom.saturating_add_signed(step).min(MAX_ZOOM);
    if zoom != view.zoom {
        // The point under the pointer stays where it is.
        if let Some(p) = towards {
            let offset = p - rect.center();
            let (ox, oy) = (f64::from(offset.x) / span, f64::from(offset.y) / span);
            let factor = tiles::span(view.zoom) / tiles::span(zoom);
            view.centre.x += ox * (1. - factor);
            view.centre.y += oy * (1. - factor);
        }
        view.zoom = zoom;
    }
    view.centre.x = view.centre.x.rem_euclid(1.);
    view.centre.y = view.centre.y.clamp(0., 1.);
}

/// The credit the tiles carry, in the map's bottom right corner, linked to
/// OpenStreetMap's copyright page when it is OpenStreetMap's.
fn credit(ui: &mut egui::Ui, rect: egui::Rect, credit: &str) {
    if credit.is_empty() {
        return;
    }
    let palette = theme::palette(ui.ctx());
    let font = egui::FontId::proportional(text::MICRO);
    let galley = ui
        .painter()
        .layout_no_wrap(credit.to_owned(), font, palette.tone(Tone::TEXT));
    let pad = Vec2::new(space::XS, space::HAIR);
    let at = egui::Rect::from_min_max(
        rect.right_bottom() - galley.size() - 2. * pad - Vec2::splat(space::HAIR),
        rect.right_bottom() - Vec2::splat(space::HAIR),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        at,
        radius::SMALL,
        palette.tone(Tone::CARD).gamma_multiply(0.85),
    );
    painter.galley(at.min + pad, galley, palette.tone(Tone::TEXT));
    if credit.contains("OpenStreetMap") {
        let link = ui
            .interact(at, ui.id().with("map-credit"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(tiles::OPENSTREETMAP_COPYRIGHT);
        if link.clicked() {
            let _ = crate::platform::web::open(tiles::OPENSTREETMAP_COPYRIGHT);
        }
    }
}

/// openstreetmap.org at the map's view: with a marker on the place when there
/// is one place.
fn browser_url(places: &[(f64, f64)], zoom: u8, lat: f64, lon: f64) -> String {
    match places {
        [(plat, plon)] => format!(
            "https://www.openstreetmap.org/?mlat={plat:.6}&mlon={plon:.6}#map={zoom}/{lat:.6}/{lon:.6}"
        ),
        _ => format!("https://www.openstreetmap.org/#map={zoom}/{lat:.6}/{lon:.6}"),
    }
}

#[cfg(test)]
#[path = "map_tests.rs"]
mod tests;
