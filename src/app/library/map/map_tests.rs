use super::*;
use crate::app::library::Library;
use crate::metadata::{Descriptive, Location};

#[test]
fn the_map_is_off_until_turned_on_and_an_address_that_cant_be_used_shows_none() {
    let d = Settings::default();
    assert!(!d.on && d.source().is_none());
    assert_eq!(
        d.to_session(),
        (false, None, None),
        "OpenStreetMap's is saved as none"
    );
    let on = Settings { on: true, ..d };
    let source = on.source().unwrap();
    assert_eq!(
        source.template.url(TileId { z: 1, x: 0, y: 1 }),
        "https://tile.openstreetmap.org/1/0/1.png"
    );
    assert_eq!(source.credit, tiles::OPENSTREETMAP_CREDIT);
    let plain = Settings {
        tiles: "http://tiles.example.org/{z}/{x}/{y}.png".into(),
        ..on
    };
    assert!(plain.source().is_none() && plain.problem().is_some());
    let own = Settings::of(
        true,
        Some(" https://tiles.example.org/{z}/{x}/{y}.png "),
        Some(""),
    );
    assert_eq!(own.tiles, "https://tiles.example.org/{z}/{x}/{y}.png");
    assert_eq!(
        own.credit,
        tiles::OPENSTREETMAP_CREDIT,
        "an empty credit is OpenStreetMap's"
    );
    assert_eq!(
        own.to_session(),
        (
            true,
            Some("https://tiles.example.org/{z}/{x}/{y}.png".into()),
            None
        )
    );
}

#[test]
fn the_browser_opens_on_the_place_with_a_marker_or_on_the_view_of_several() {
    assert_eq!(
        browser_url(&[(52.5, 13.4)], 15, 52.5, 13.4),
        "https://www.openstreetmap.org/?mlat=52.500000&mlon=13.400000#map=15/52.500000/13.400000"
    );
    assert_eq!(
        browser_url(&[(52.5, 13.4), (52.2, 21.0)], 6, 52.3, 17.2),
        "https://www.openstreetmap.org/#map=6/52.300000/17.200000"
    );
}

/// A Library over one photo taken at Berlin's Brandenburg Gate, shown in the
/// Metadata panel, with the map's tiles kept in `tiles`.
fn library_at_the_gate(tiles: &std::path::Path) -> (tempfile::TempDir, Library) {
    let (d, mut e, ids) = crate::app::testing::editor_with_catalog(&["a.ARW"]).unwrap();
    let mut library = *e.library.take().unwrap();
    locate(&mut library, ids[0], 52.516_275, 13.377_704);
    library.select(Some(ids[0]));
    library.map = Map::for_tests(tiles.to_path_buf());
    (d, library)
}
fn locate(library: &mut Library, id: crate::catalog::PhotoId, lat: f64, lon: f64) {
    let catalog = library.session.catalog_mut_for_tests();
    let mut snapshot = catalog.metadata_snapshot(&[id]).unwrap().remove(0);
    snapshot.descriptive = Descriptive {
        location: Some(Location::At {
            lat,
            lon,
            alt: None,
        }),
        ..catalog.descriptive(id).unwrap()
    };
    catalog.restore_metadata(&[snapshot]).unwrap();
    library.fields.reload();
}
fn frame(library: &mut Library, ctx: &egui::Context) {
    let mut output = ctx.run_ui(Default::default(), |ui| {
        egui::Panel::right("info").exact_size(270.).show(ui, |ui| {
            library.metadata_fields(ui);
        });
    });
    output.textures_delta.clear();
}
/// A plain HTTP tile server counting what it is asked for.
fn tile_server() -> (u16, Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{BufRead, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let a = asked.clone();
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(256, 256, image::Rgba([200, 220, 200, 255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let png = png.into_inner();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (a, png) = (a.clone(), png.clone());
            std::thread::spawn(move || {
                let mut out = stream.try_clone().unwrap();
                let mut reader = std::io::BufReader::new(stream);
                loop {
                    let mut first = String::new();
                    if reader.read_line(&mut first).unwrap_or(0) == 0 {
                        return;
                    }
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        if line.trim().is_empty() {
                            break;
                        }
                    }
                    a.lock()
                        .unwrap()
                        .push(first.split_whitespace().nth(1).unwrap_or("").into());
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", png.len());
                    if out.write_all(head.as_bytes()).is_err() || out.write_all(&png).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (port, asked)
}
fn source(port: u16) -> Source {
    Source {
        template: Arc::new(Template::unchecked(&format!(
            "http://127.0.0.1:{port}/{{z}}/{{x}}/{{y}}.png"
        ))),
        credit: tiles::OPENSTREETMAP_CREDIT.into(),
    }
}
fn settle(library: &mut Library, ctx: &egui::Context, until: impl Fn(&Library) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !until(library) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        frame(library, ctx);
    }
}

#[test]
fn nothing_is_asked_for_while_the_map_is_off_and_the_place_s_tile_first_once_on() {
    let tiles = tempfile::tempdir().unwrap();
    let (_d, mut library) = library_at_the_gate(tiles.path());
    let ctx = egui::Context::default();
    let (port, asked) = tile_server();
    for _ in 0..3 {
        frame(&mut library, &ctx);
    }
    assert!(
        library.map.fetcher.is_none(),
        "no worker, no request, while off"
    );
    library.set_map(Some(source(port)));
    frame(&mut library, &ctx);
    settle(&mut library, &ctx, |l| !l.map.textures.is_empty());
    // The gate is on tile 15/17601/10746 of OpenStreetMap.
    // The two workers take the first two asked for; the place's own tile is one.
    let first: Vec<String> = asked.lock().unwrap().iter().take(2).cloned().collect();
    assert!(
        first.iter().any(|p| p == "/15/17601/10746.png"),
        "{first:?}"
    );
    assert!(library.map.textures.contains_key(&TileId {
        z: 15,
        x: 17601,
        y: 10746
    }));
    // A photo without a place shows no map, and asks for nothing more.
    library.set_map(None);
    frame(&mut library, &ctx);
    std::thread::sleep(fetch::WANTED_FOR * 2);
    let before = asked.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(asked.lock().unwrap().len(), before);
}

#[test]
fn tiles_of_a_server_no_longer_chosen_are_dropped_when_they_arrive() {
    let tiles = tempfile::tempdir().unwrap();
    let (_d, mut library) = library_at_the_gate(tiles.path());
    let ctx = egui::Context::default();
    let (old, asked_old) = tile_server();
    let (new, asked_new) = tile_server();
    library.set_map(Some(source(old)));
    frame(&mut library, &ctx);
    // The old server answers; its tiles are on their way back, undrawn.
    let deadline = Instant::now() + Duration::from_secs(10);
    while asked_old.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(200));
    library.set_map(Some(source(new)));
    frame(&mut library, &ctx);
    assert!(
        library.map.textures.is_empty(),
        "the old server's tiles are not drawn"
    );
    settle(&mut library, &ctx, |l| !l.map.textures.is_empty());
    assert!(!asked_new.lock().unwrap().is_empty());
}

#[test]
fn several_photos_far_apart_are_shown_together() {
    let tiles = tempfile::tempdir().unwrap();
    let (_d, mut e, ids) = crate::app::testing::editor_with_catalog(&["a.ARW", "b.ARW"]).unwrap();
    let mut library = *e.library.take().unwrap();
    locate(&mut library, ids[0], 52.52, 13.40);
    locate(&mut library, ids[1], 52.23, 21.01);
    library.select_all();
    library.map = Map::for_tests(tiles.path().to_path_buf());
    library.map.set_source(Some(source(1)));
    let ctx = egui::Context::default();
    frame(&mut library, &ctx);
    assert_eq!(library.fields.places.len(), 2);
    let view = library.map.view.as_ref().unwrap();
    assert!(view.zoom <= 6, "both fit: {}", view.zoom);
}

/// A made-up street map tile for the shots: paper, a grid of streets, a park
/// and a river, drawn from the tile's place so neighbours join up.
fn made_up_tile(tile: TileId) -> egui::ColorImage {
    let mut pixels = Vec::with_capacity(256 * 256);
    for y in 0..256u32 {
        for x in 0..256u32 {
            let wx = (tile.x * 256 + x) as f32;
            let wy = (tile.y * 256 + y) as f32;
            let street = (wx % 96.) < 5. || (wy % 80.) < 5.;
            let main = ((wx + wy * 0.4) % 400.) < 9.;
            let river = ((wy - (wx * 0.05).sin() * 30. - wx * 0.3) % 1100.).abs() < 26.;
            let park = ((wx / 300.) as i32 + (wy / 260.) as i32) % 5 == 0;
            let c = if river {
                [170, 211, 223]
            } else if main {
                [252, 214, 164]
            } else if street {
                [255, 255, 255]
            } else if park {
                [205, 235, 176]
            } else {
                [242, 239, 233]
            };
            pixels.push(Color32::from_rgb(c[0], c[1], c[2]));
        }
    }
    egui::ColorImage::new([256, 256], pixels)
}
/// The tiles around `centre` at zooms near `zoom`, so the map draws them
/// whichever it fits the places at.
fn near(centre: World, zoom: u8) -> Vec<TileId> {
    (zoom.saturating_sub(1)..=zoom + 1)
        .flat_map(|z| tiles::visible(centre, z, 700., 700.))
        .map(|t| t.0)
        .collect()
}
fn fill_tiles(library: &mut Library, ctx: &egui::Context, centre: World, zoom: u8) {
    for tile in near(centre, zoom) {
        let texture = ctx.load_texture(
            "shot-tile",
            made_up_tile(tile),
            egui::TextureOptions::LINEAR,
        );
        library.map.textures.insert(tile, texture);
    }
}

/// Draws the Metadata panel with its map, for looking at, when
/// `RAWMAKASE_UI_SHOTS` names a folder ([`crate::app::ui::shots`]).
#[test]
fn shots_of_the_metadata_map() {
    let shots = [
        ("metadata-map", 1usize, false),
        ("metadata-map-two-places", 2, false),
        ("metadata-map-unavailable", 1, true),
        ("metadata-map-off", 1, false),
    ];
    for (name, places, failing) in shots {
        crate::app::ui::shots::shot(
            name,
            egui::vec2(300., 440.),
            move |e, ctx| {
                let tiles = tempfile::tempdir().unwrap().keep();
                let (d, mut editor, ids) =
                    crate::app::testing::editor_with_catalog(&["L1051597.DNG", "L1051598.DNG"])
                        .unwrap();
                std::mem::forget(d);
                let mut library = *editor.library.take().unwrap();
                locate(&mut library, ids[0], 52.516_275, 13.377_704);
                if places == 2 {
                    locate(&mut library, ids[1], 52.520_8, 13.409_4);
                    library.select_all();
                } else {
                    library.select(Some(ids[0]));
                }
                library.map = Map::for_tests(tiles);
                if name != "metadata-map-off" {
                    library.map.set_source(Some(Source {
                        template: Arc::new(
                            Template::parse("https://tiles.invalid/{z}/{x}/{y}.png").unwrap(),
                        ),
                        credit: tiles::OPENSTREETMAP_CREDIT.into(),
                    }));
                }
                library.sync_fields();
                let world: Vec<World> = library
                    .fields
                    .places
                    .iter()
                    .map(|(a, b)| World::of(*a, *b))
                    .collect();
                let zoom = tiles::fit(&world, 270. - 32., 150., MARGIN, PLACE_ZOOM);
                let centre = tiles::centre(&world).unwrap();
                if failing {
                    for tile in near(centre, zoom) {
                        library
                            .map
                            .failed
                            .insert(tile, (Instant::now(), "the map server answered 503".into()));
                    }
                } else {
                    fill_tiles(&mut library, ctx, centre, zoom);
                }
                e.library = Some(Box::new(library));
            },
            |e, ctx| {
                crate::app::ui::shots::panel(ctx, 270., |ui| {
                    ui.spacing_mut().item_spacing.y = 0.;
                    e.library.as_mut().unwrap().metadata_fields(ui);
                });
            },
        );
    }
}

#[test]
fn a_session_without_the_map_reads_as_off_and_a_chosen_server_survives_a_save() {
    use crate::app::preferences::session::Session;
    let old: Session = serde_json::from_str("{}").unwrap();
    assert_eq!(
        Settings::of(old.map, old.map_tiles.as_deref(), old.map_credit.as_deref()),
        Settings::default()
    );
    let chosen = Settings::of(
        true,
        Some("https://tiles.example.org/{z}/{x}/{y}.png"),
        Some("© Example"),
    );
    let (map, map_tiles, map_credit) = chosen.to_session();
    let json = serde_json::to_string(&Session {
        map,
        map_tiles,
        map_credit,
        ..Default::default()
    })
    .unwrap();
    let read: Session = serde_json::from_str(&json).unwrap();
    assert_eq!(
        Settings::of(
            read.map,
            read.map_tiles.as_deref(),
            read.map_credit.as_deref()
        ),
        chosen
    );
    // OpenStreetMap's own, and off, write nothing.
    let json = serde_json::to_string(&Session::default()).unwrap();
    assert!(!json.contains("map"), "{json}");
}
