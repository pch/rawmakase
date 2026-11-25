//! Map tiles: where a place falls on the Web Mercator map that slippy-map
//! servers draw, which tiles a view needs, where a tile is kept on disk and how
//! long it may be used before it is asked for again. No drawing and no network.
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// OpenStreetMap's own tile server, which its tile usage policy asks to be
/// reached by exactly this address: HTTPS, no `a.`/`b.`/`c.` subdomains.
pub(in crate::app) const OPENSTREETMAP: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
/// The credit OpenStreetMap's tiles must carry on the map.
pub(in crate::app) const OPENSTREETMAP_CREDIT: &str = "© OpenStreetMap contributors";
pub(in crate::app) const OPENSTREETMAP_COPYRIGHT: &str = "https://www.openstreetmap.org/copyright";
/// The width and height of a tile, in pixels; every slippy-map server uses it.
pub(super) const TILE: f64 = 256.;
/// The deepest zoom OpenStreetMap draws.
pub(super) const MAX_ZOOM: u8 = 19;
/// How long a tile is used without asking again when the server says nothing,
/// the shortest OpenStreetMap's policy allows.
pub(super) const DEFAULT_FRESH: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// One tile of the map: `x` and `y` count from the top left at zoom `z`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

/// A tile server's address, with `{z}`, `{x}` and `{y}` where a tile's
/// numbers go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Template(String);
impl Template {
    /// The address, if it is HTTPS and names all three numbers. Tiles are never
    /// fetched over plain HTTP: the request tells where the photos were taken.
    pub(in crate::app) fn parse(text: &str) -> Result<Self, &'static str> {
        let text = text.trim();
        if !text.starts_with("https://") {
            return Err("The address must start with https://");
        }
        if !["{z}", "{x}", "{y}"].iter().all(|p| text.contains(p)) {
            return Err("The address must contain {z}, {x} and {y}");
        }
        Ok(Self(text.to_owned()))
    }
    /// A template taken as it is, for tests against a local server.
    #[cfg(test)]
    pub(super) fn unchecked(text: &str) -> Self {
        Self(text.to_owned())
    }
    pub(super) fn url(&self, tile: TileId) -> String {
        self.0
            .replace("{z}", &tile.z.to_string())
            .replace("{x}", &tile.x.to_string())
            .replace("{y}", &tile.y.to_string())
    }
    /// The folder of `root` this server's tiles are kept in, named by a hash of
    /// its address so two servers' tiles never mix.
    pub(super) fn folder(&self, root: &Path) -> PathBuf {
        use sha2::Digest;
        let hash = sha2::Sha256::digest(self.0.as_bytes());
        let name: String = hash[..8].iter().map(|b| format!("{b:02x}")).collect();
        root.join(name)
    }
}

/// A place on the map at zoom 0, from 0 to 1 across and down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct World {
    pub x: f64,
    pub y: f64,
}
/// The deepest latitude Web Mercator draws; beyond it the map ends.
const MAX_LATITUDE: f64 = 85.051_128_779_806_59;
impl World {
    pub(super) fn of(lat: f64, lon: f64) -> Self {
        let lat = lat.clamp(-MAX_LATITUDE, MAX_LATITUDE).to_radians();
        Self {
            x: (lon + 180.) / 360.,
            y: (1. - (lat.tan() + 1. / lat.cos()).ln() / std::f64::consts::PI) / 2.,
        }
    }
    /// Latitude and longitude, in degrees.
    pub(super) fn degrees(self) -> (f64, f64) {
        let n = std::f64::consts::PI * (1. - 2. * self.y);
        (n.sinh().atan().to_degrees(), self.x * 360. - 180.)
    }
}
/// The width of the whole map at `zoom`, in pixels.
pub(super) fn span(zoom: u8) -> f64 {
    TILE * f64::from(1u32 << zoom)
}

/// The deepest zoom, up to `max`, at which every place fits in a view `width`
/// by `height` pixels, keeping `margin` pixels clear at its edges.
pub(super) fn fit(places: &[World], width: f64, height: f64, margin: f64, max: u8) -> u8 {
    let (Some(min_x), Some(max_x), Some(min_y), Some(max_y)) = (
        places.iter().map(|p| p.x).reduce(f64::min),
        places.iter().map(|p| p.x).reduce(f64::max),
        places.iter().map(|p| p.y).reduce(f64::min),
        places.iter().map(|p| p.y).reduce(f64::max),
    ) else {
        return max;
    };
    let room = (
        (width - 2. * margin).max(1.),
        (height - 2. * margin).max(1.),
    );
    (0..=max)
        .rev()
        .find(|z| (max_x - min_x) * span(*z) <= room.0 && (max_y - min_y) * span(*z) <= room.1)
        .unwrap_or(0)
}
/// The middle of the places' bounds.
pub(super) fn centre(places: &[World]) -> Option<World> {
    let fold = |f: fn(f64, f64) -> f64, g: fn(&World) -> f64| places.iter().map(g).reduce(f);
    Some(World {
        x: (fold(f64::min, |p| p.x)? + fold(f64::max, |p| p.x)?) / 2.,
        y: (fold(f64::min, |p| p.y)? + fold(f64::max, |p| p.y)?) / 2.,
    })
}

/// The tiles a view `width` by `height` pixels around `centre` at `zoom`
/// shows, nearest the middle first, so the place itself is fetched first. The
/// map repeats across; it ends at the top and bottom.
pub(super) fn visible(centre: World, zoom: u8, width: f64, height: f64) -> Vec<(TileId, f64, f64)> {
    let n = 1i64 << zoom;
    let span = span(zoom);
    let (left, top) = (centre.x * span - width / 2., centre.y * span - height / 2.);
    let columns = (left / TILE).floor() as i64..=((left + width) / TILE).floor() as i64;
    let rows = (top / TILE).floor().max(0.) as i64
        ..=((top + height) / TILE).floor().min(n as f64 - 1.) as i64;
    let mut tiles = Vec::new();
    for row in rows {
        for column in columns.clone() {
            let tile = TileId {
                z: zoom,
                x: column.rem_euclid(n) as u32,
                y: row as u32,
            };
            tiles.push((tile, column as f64 * TILE - left, row as f64 * TILE - top));
        }
    }
    let middle = |(_, x, y): &(TileId, f64, f64)| {
        let (dx, dy) = (x + TILE / 2. - width / 2., y + TILE / 2. - height / 2.);
        dx * dx + dy * dy
    };
    tiles.sort_by(|a, b| middle(a).total_cmp(&middle(b)));
    tiles
}

/// Where a tile and what the server said about it are kept.
pub(super) fn paths(folder: &Path, tile: TileId) -> (PathBuf, PathBuf) {
    let dir = folder.join(tile.z.to_string()).join(tile.x.to_string());
    (
        dir.join(format!("{}.tile", tile.y)),
        dir.join(format!("{}.json", tile.y)),
    )
}

/// What the server said about a tile it sent, kept beside it so an expired
/// tile is asked for again only if it changed.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct Meta {
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
    /// Seconds since 1970 until which the tile is used without asking.
    #[serde(default)]
    pub fresh_until: u64,
}
impl Meta {
    /// From a response's headers, received at `now`: `Cache-Control: max-age`
    /// first, then `Expires`, else [`DEFAULT_FRESH`].
    pub(super) fn of(
        now: SystemTime,
        cache_control: Option<&str>,
        expires: Option<&str>,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Self {
        let max_age = cache_control.and_then(|c| {
            c.split(',')
                .filter_map(|d| d.trim().strip_prefix("max-age="))
                .find_map(|s| s.trim_matches('"').parse::<u64>().ok())
        });
        let now_secs = seconds(now);
        let fresh_until = match (max_age, expires.and_then(http_date)) {
            (Some(age), _) => now_secs.saturating_add(age),
            (None, Some(at)) => at,
            (None, None) => now_secs + DEFAULT_FRESH.as_secs(),
        };
        Self {
            etag: etag.map(str::to_owned),
            last_modified: last_modified.map(str::to_owned),
            fresh_until,
        }
    }
    pub(super) fn fresh(&self, now: SystemTime) -> bool {
        seconds(now) < self.fresh_until
    }
}
fn seconds(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}
/// An HTTP date (`Sun, 06 Nov 1994 08:49:37 GMT`), as seconds since 1970.
fn http_date(text: &str) -> Option<u64> {
    let mut parts = text.split_whitespace().skip(1);
    let day: u64 = parts.next()?.parse().ok()?;
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| Some(*m) == parts.clone().next())? as u64
        + 1;
    parts.next();
    let year: u64 = parts.next()?.parse().ok()?;
    let mut time = parts.next()?.split(':').map(|p| p.parse::<u64>().ok());
    let (h, m, s) = (time.next()??, time.next()??, time.next()??);
    if parts.next()? != "GMT" || !(1..=31).contains(&day) || year < 1970 {
        return None;
    }
    // Days from 1970-01-01 to the date, by the civil-from-days algorithm.
    let (y, mo) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = y / 400;
    let yoe = y - era * 400;
    let doy = (153 * mo + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = (era * 146_097 + doe).checked_sub(719_468)?;
    Some(days * 86_400 + h * 3600 + m * 60 + s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_falls_on_the_tile_slippy_maps_draw_it_on() {
        let middle = World::of(0., 0.);
        assert!((middle.x - 0.5).abs() < 1e-12 && (middle.y - 0.5).abs() < 1e-12);
        // Berlin's Brandenburg Gate is on tile 10/550/335 of OpenStreetMap.
        let gate = World::of(52.516_275, 13.377_704);
        let at = |w: World, z: u8| ((w.x * span(z) / TILE) as u32, (w.y * span(z) / TILE) as u32);
        assert_eq!(at(gate, 10), (550, 335));
        let (lat, lon) = gate.degrees();
        assert!((lat - 52.516_275).abs() < 1e-9 && (lon - 13.377_704).abs() < 1e-9);
        // The poles are drawn at the map's edge, never beyond it.
        assert!(World::of(90., 0.).y.abs() < 1e-9);
    }

    #[test]
    fn one_place_takes_the_deepest_zoom_and_far_apart_ones_a_shallower_one() {
        let berlin = World::of(52.52, 13.40);
        assert_eq!(fit(&[berlin], 270., 170., 20., 15), 15);
        let warsaw = World::of(52.23, 21.01);
        let both = fit(&[berlin, warsaw], 270., 170., 20., 15);
        assert!(both <= 6, "{both}");
        let c = centre(&[berlin, warsaw]).unwrap();
        assert!((c.x - (berlin.x + warsaw.x) / 2.).abs() < 1e-12);
        assert_eq!(centre(&[]), None);
    }

    #[test]
    fn a_view_asks_for_the_tiles_it_shows_middle_first_and_wraps_across() {
        let tiles = visible(World { x: 0.5, y: 0.5 }, 2, 256., 256.);
        // Four tiles meet in the middle of the map at zoom 2.
        let ids: std::collections::HashSet<_> = tiles.iter().map(|t| (t.0.x, t.0.y)).collect();
        assert_eq!(ids, [(1, 1), (2, 1), (1, 2), (2, 2)].into_iter().collect());
        // At the date line the tiles from the other side of the map are used.
        let edge = visible(World { x: 0., y: 0.5 }, 1, 256., 100.);
        assert!(edge.iter().any(|t| t.0.x == 1) && edge.iter().any(|t| t.0.x == 0));
        // None below the map's bottom edge.
        let bottom = visible(World { x: 0.5, y: 1. }, 1, 256., 256.);
        assert!(bottom.iter().all(|t| t.0.y < 2));
        let middle_first = visible(World::of(52.52, 13.40), 15, 270., 170.);
        let gate = World::of(52.52, 13.40);
        let want = (
            (gate.x * span(15) / TILE) as u32,
            (gate.y * span(15) / TILE) as u32,
        );
        assert_eq!((middle_first[0].0.x, middle_first[0].0.y), want);
    }

    #[test]
    fn only_https_addresses_naming_every_number_are_taken() {
        assert!(Template::parse(OPENSTREETMAP).is_ok());
        assert!(Template::parse("http://tile.openstreetmap.org/{z}/{x}/{y}.png").is_err());
        assert!(Template::parse("https://example.org/{z}/{x}.png").is_err());
        let t = Template::parse("  https://tiles.example.org/{z}/{x}/{y}.png ").unwrap();
        assert_eq!(
            t.url(TileId { z: 3, x: 4, y: 5 }),
            "https://tiles.example.org/3/4/5.png"
        );
        let root = Path::new("/cache");
        assert_ne!(
            t.folder(root),
            Template::parse(OPENSTREETMAP).unwrap().folder(root)
        );
    }

    #[test]
    fn a_tile_is_used_as_long_as_its_server_says_and_a_week_when_it_says_nothing() {
        let now = UNIX_EPOCH + Duration::from_secs(800_000_000);
        // max-age wins over Expires.
        let aged = Meta::of(
            now,
            Some("public, max-age=3600"),
            Some("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some("\"a\""),
            None,
        );
        assert_eq!(aged.fresh_until, 800_003_600);
        assert_eq!(aged.etag.as_deref(), Some("\"a\""));
        let expires = Meta::of(now, None, Some("Sun, 06 Nov 1994 08:49:37 GMT"), None, None);
        assert_eq!(expires.fresh_until, 784_111_777);
        assert!(!expires.fresh(now));
        // A max-age past the end of time is kept until then, not wrapped round.
        let forever = Meta::of(now, Some("max-age=18446744073709551615"), None, None, None);
        assert_eq!(forever.fresh_until, u64::MAX);
        let silent = Meta::of(now, Some("public"), Some("never"), None, None);
        assert_eq!(silent.fresh_until, 800_000_000 + 7 * 86_400);
        assert!(silent.fresh(now));
    }
}
