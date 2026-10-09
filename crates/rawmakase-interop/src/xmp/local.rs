//! Lightroom's spot removal, red eye corrections and masks (`RetouchAreas`, the legacy
//! `RetouchInfo`, `RedEyeInfo` and the mask correction lists) from XMP or a Lightroom
//! catalog, as retouch operations, red eye corrections and masks.
//!
//! Conventions, checked against Camera Raw 18.6 renders of a landscape photo and of
//! the same photo tagged as portrait: positions are normalised to the camera's default
//! crop in the *unrotated* sensor frame; sizes are fractions of the long edge; a spot's
//! source is the absolute position of its centre (`SourceX`, `OffsetY`), and a brushed
//! spot's source is where its first dab copies from. Gradient, radial and brush masks
//! are assumed to use the same frame and units; their shape details are not verified.
use crate::model::image_frame::ImageFrame;
use crate::model::masks::{
    BrushStroke, LocalAdjust, MAX_GROUPS, MaskComponent, MaskGroup, MaskOp, MaskShape,
};
use crate::model::red_eye::{EyeKind, RedEyeOp};
use crate::model::retouch::{RetouchMode, RetouchOp, RetouchShape};
use anyhow::{Context, Result, bail, ensure};
use std::collections::BTreeMap;

/// The Lightroom settings this module reads.
pub const KEYS: [&str; 7] = [
    "RetouchAreas",
    "RetouchInfo",
    "RedEyeInfo",
    "MaskGroupBasedCorrections",
    "PaintBasedCorrections",
    "GradientBasedCorrections",
    "CircularGradientBasedCorrections",
];
/// The mask correction lists among [`KEYS`].
const MASK_KEYS: [&str; 4] = [
    "MaskGroupBasedCorrections",
    "PaintBasedCorrections",
    "GradientBasedCorrections",
    "CircularGradientBasedCorrections",
];

/// Adobe's nested settings as data: text, lists (`rdf:Seq`, Lua arrays) and records
/// (`rdf:Description`, Lua keyed tables).
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Text(String),
    List(Vec<Node>),
    Record(BTreeMap<String, Node>),
}
impl Node {
    fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Record(r) => r.get(key),
            _ => None,
        }
    }
    fn text(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Node::Text(t) => Some(t),
            _ => None,
        }
    }
    fn num(&self, key: &str) -> Option<f32> {
        self.text(key)?
            .trim()
            .parse()
            .ok()
            .filter(|v: &f32| v.is_finite())
    }
    fn flag(&self, key: &str) -> Option<bool> {
        match self.text(key)?.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        }
    }
    fn items(&self) -> &[Node] {
        match self {
            Node::List(items) => items,
            _ => &[],
        }
    }
    pub fn is_empty(&self) -> bool {
        match self {
            Node::Text(t) => t.trim().is_empty(),
            Node::List(l) => l.is_empty(),
            Node::Record(r) => r.is_empty(),
        }
    }
    /// A CRS element's value: a list for `rdf:Seq`/`Bag`/`Alt`, a record for an
    /// `rdf:Description` or an element with CRS attributes, text otherwise.
    pub(crate) fn from_xml(node: roxmltree::Node<'_, '_>) -> Node {
        use crate::xml::ns::{CRS, RDF};
        if let Some(seq) = node.children().find(|n| {
            n.has_tag_name((RDF, "Seq"))
                || n.has_tag_name((RDF, "Bag"))
                || n.has_tag_name((RDF, "Alt"))
        }) {
            return Node::List(
                seq.children()
                    .filter(|n| n.has_tag_name((RDF, "li")))
                    .map(Node::from_xml)
                    .collect(),
            );
        }
        if let Some(d) = node
            .children()
            .find(|n| n.has_tag_name((RDF, "Description")))
        {
            return Node::from_xml(d);
        }
        let attrs: Vec<_> = node
            .attributes()
            .filter(|a| a.namespace() == Some(CRS))
            .collect();
        let children: Vec<_> = node
            .children()
            .filter(|n| n.is_element() && n.tag_name().namespace() == Some(CRS))
            .collect();
        if attrs.is_empty() && children.is_empty() {
            return Node::Text(node.text().unwrap_or("").trim().to_string());
        }
        let mut record: BTreeMap<String, Node> = attrs
            .into_iter()
            .map(|a| (a.name().to_string(), Node::Text(a.value().to_string())))
            .collect();
        for c in children {
            record.insert(c.tag_name().name().to_string(), Node::from_xml(c));
        }
        Node::Record(record)
    }
    /// A Lua table literal as Lightroom serialises develop settings; data only.
    pub fn from_lua(text: &str) -> Result<Node> {
        let mut p = Lua {
            s: text.as_bytes(),
            i: 0,
            depth: 0,
        };
        let node = p.value()?;
        p.space();
        ensure!(p.i == p.s.len(), "Trailing text after Lightroom table");
        Ok(node)
    }
}
struct Lua<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}
impl Lua<'_> {
    fn space(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.space();
        self.s.get(self.i).copied()
    }
    fn value(&mut self) -> Result<Node> {
        match self.peek().context("Unexpected end of Lightroom table")? {
            b'{' => self.table(),
            b'"' | b'\'' => self.string().map(Node::Text),
            _ => {
                let start = self.i;
                while self.i < self.s.len()
                    && !matches!(self.s[self.i], b',' | b'}' | b'=')
                    && !self.s[self.i].is_ascii_whitespace()
                {
                    self.i += 1;
                }
                let token = std::str::from_utf8(&self.s[start..self.i])?;
                ensure!(
                    token == "true" || token == "false" || token.parse::<f64>().is_ok(),
                    "Unsupported Lightroom value {token}"
                );
                Ok(Node::Text(token.to_string()))
            }
        }
    }
    fn string(&mut self) -> Result<String> {
        let quote = self.s[self.i];
        self.i += 1;
        let mut out = Vec::new();
        while let Some(&c) = self.s.get(self.i) {
            self.i += 1;
            match c {
                b'\\' => {
                    let e = *self.s.get(self.i).context("Unterminated escape")?;
                    self.i += 1;
                    out.push(match e {
                        b'n' => b'\n',
                        b't' => b'\t',
                        other => other,
                    });
                }
                c if c == quote => return Ok(String::from_utf8(out)?),
                c => out.push(c),
            }
        }
        bail!("Unterminated Lightroom string")
    }
    fn table(&mut self) -> Result<Node> {
        self.depth += 1;
        ensure!(self.depth < 64, "Lightroom table nesting too deep");
        self.i += 1;
        let (mut list, mut record) = (Vec::new(), BTreeMap::new());
        loop {
            match self.peek().context("Unterminated Lightroom table")? {
                b'}' => {
                    self.i += 1;
                    break;
                }
                b',' | b';' => self.i += 1,
                _ => {
                    // `key = value`, or a positional value.
                    let save = self.i;
                    let mut key = None;
                    if self.s[self.i].is_ascii_alphabetic() || self.s[self.i] == b'_' {
                        let start = self.i;
                        while self.i < self.s.len()
                            && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_')
                        {
                            self.i += 1;
                        }
                        let name = std::str::from_utf8(&self.s[start..self.i])?.to_string();
                        if self.peek() == Some(b'=') {
                            self.i += 1;
                            key = Some(name);
                        } else {
                            self.i = save;
                        }
                    }
                    let v = self.value()?;
                    match key {
                        Some(k) => {
                            record.insert(k, v);
                        }
                        None => list.push(v),
                    }
                }
            }
        }
        self.depth -= 1;
        Ok(if record.is_empty() {
            Node::List(list)
        } else {
            Node::Record(record)
        })
    }
}

/// Lightroom's local edits of one photo, converted.
#[derive(Debug, Default)]
pub struct ConvertedLocal {
    /// `None` when the settings have no spot removal.
    pub retouch: Option<Vec<RetouchOp>>,
    /// `None` when the settings have no red eye corrections.
    pub red_eye: Option<Vec<RedEyeOp>>,
    /// `None` when the settings have no masks.
    pub masks: Option<Vec<MaskGroup>>,
    /// What could not be converted, for the user.
    pub skipped: Vec<String>,
}
/// Converts positions from Lightroom's frame to image space.
struct Frame {
    image: ImageFrame,
    /// Unrotated width and height over the long edge.
    scale: [f32; 2],
}
impl Frame {
    fn new(image: ImageFrame) -> Self {
        let [w, h] = image.size();
        let (w, h) = if image.turns % 2 == 1 { (h, w) } else { (w, h) };
        let long = w.max(h);
        Self {
            image,
            scale: [w / long, h / long],
        }
    }
    fn point(&self, x: f32, y: f32) -> [f32; 2] {
        self.image.from_unrotated([x, y])
    }
}
pub fn convert(local: &BTreeMap<String, Node>, image: ImageFrame) -> ConvertedLocal {
    let frame = Frame::new(image);
    let mut edits = ConvertedLocal::default();
    if let Some(areas) = local.get("RetouchAreas").filter(|n| !n.is_empty()) {
        edits.retouch = Some(retouch_areas(areas, &frame, &mut edits.skipped));
    } else if let Some(info) = local.get("RetouchInfo").filter(|n| !n.is_empty()) {
        edits.retouch = Some(retouch_info(info, &frame, &mut edits.skipped));
    }
    if let Some(info) = local.get("RedEyeInfo").filter(|n| !n.is_empty()) {
        edits.red_eye = Some(red_eye_info(info, &frame, &mut edits.skipped));
    }
    let mut groups = Vec::new();
    let mut any = false;
    for key in &MASK_KEYS {
        if let Some(list) = local.get(*key).filter(|n| !n.is_empty()) {
            any = true;
            for c in list.items() {
                match correction(c, &frame) {
                    Ok(Some(g)) if groups.len() < MAX_GROUPS => groups.push(g),
                    Ok(Some(g)) => edits
                        .skipped
                        .push(format!("{}: more than {} masks", g.name, MAX_GROUPS)),
                    Ok(None) => {}
                    Err(e) => {
                        let name = c.text("CorrectionName").unwrap_or("A mask");
                        edits.skipped.push(format!("{name}: {e:#}"));
                    }
                }
            }
        }
    }
    if any {
        edits.masks = Some(groups);
    }
    edits
}
fn retouch_areas(areas: &Node, frame: &Frame, skipped: &mut Vec<String>) -> Vec<RetouchOp> {
    let mut ops = Vec::new();
    for (i, area) in areas.items().iter().enumerate() {
        match retouch_area(area, frame) {
            Ok(op) => ops.push(op),
            Err(e) => skipped.push(format!("Spot {}: {e:#}", i + 1)),
        }
    }
    ops
}
fn retouch_area(area: &Node, frame: &Frame) -> Result<RetouchOp> {
    let mode = match area.text("SpotType").unwrap_or("heal") {
        "heal" => RetouchMode::Heal,
        "clone" => RetouchMode::Clone,
        other => bail!("{other} spots need AI (not supported)"),
    };
    let source = [
        area.num("SourceX").context("No source")?,
        area.num("OffsetY").context("No source")?,
    ];
    let mask = area
        .get("Masks")
        .and_then(|m| m.items().first())
        .context("No spot shape")?;
    let (shape, anchor) = match mask.text("What") {
        Some("Mask/Ellipse") | Some("Mask/Circle") => {
            let c = [
                mask.num("X").context("No X")?,
                mask.num("Y").context("No Y")?,
            ];
            let radius = mask.num("SizeX").context("No size")?;
            let spot = RetouchShape::Spot {
                center: frame.point(c[0], c[1]),
                radius,
            };
            (spot, c)
        }
        Some("Mask/Paint") => {
            let (strokes, radius) = dabs(mask)?;
            let points: Vec<[f32; 2]> = strokes
                .iter()
                .flatten()
                .map(|p| frame.point(p[0], p[1]))
                .collect();
            let first = strokes[0][0];
            let brush = RetouchShape::Brush {
                points: points.into(),
                radius,
            };
            (brush, first)
        }
        other => bail!("Unsupported spot shape {}", other.unwrap_or("?")),
    };
    let (a, s) = (
        frame.point(anchor[0], anchor[1]),
        frame.point(source[0], source[1]),
    );
    let feather = area.num("Feather").unwrap_or(0.);
    let op = RetouchOp {
        mode,
        shape,
        feather: if feather > 1. {
            feather / 100.
        } else {
            feather
        }
        .clamp(0., 1.),
        opacity: area.num("Opacity").unwrap_or(1.).clamp(0., 1.),
        offset: [s[0] - a[0], s[1] - a[1]],
    };
    op.validate()?;
    Ok(op)
}
/// Legacy spots: `centerX`, `centerY`, `radius`, `sourceX`, `sourceY`, `spotType`.
fn retouch_info(info: &Node, frame: &Frame, skipped: &mut Vec<String>) -> Vec<RetouchOp> {
    let mut ops = Vec::new();
    for (i, spot) in info.items().iter().enumerate() {
        let op = (|| -> Result<RetouchOp> {
            let c = [
                spot.num("centerX").context("No centre")?,
                spot.num("centerY").context("No centre")?,
            ];
            let s = [
                spot.num("sourceX").context("No source")?,
                spot.num("sourceY").context("No source")?,
            ];
            let (c, s) = (frame.point(c[0], c[1]), frame.point(s[0], s[1]));
            let op = RetouchOp {
                mode: if spot.text("spotType") == Some("clone") {
                    RetouchMode::Clone
                } else {
                    RetouchMode::Heal
                },
                shape: RetouchShape::Spot {
                    center: c,
                    radius: spot.num("radius").context("No radius")?,
                },
                feather: 0.,
                opacity: spot.num("opacity").unwrap_or(1.).clamp(0., 1.),
                offset: [s[0] - c[0], s[1] - c[1]],
            };
            op.validate()?;
            Ok(op)
        })();
        match op {
            Ok(op) => ops.push(op),
            Err(e) => skipped.push(format!("Spot {}: {e:#}", i + 1)),
        }
    }
    ops
}
/// Red eye corrections. XMP writes each as text (Camera Raw 18.7):
/// `x = 0.52, y = 0.34, width = 0.013, height = 0.019, alpha = 0, density = …,
/// strength = …, redBias = …, pupilSize = 0.5, pupilDarkenAmount = 0.5,
/// adaptivePupilColor = 0, gammaEncodeCorrection = 1, showPetEyeHighlight = 1,
/// highlightX = 0.591, highlightY = 0.424`; a catalog as a table with the ellipse in
/// `pupil.ellipse` (`centerX`, `centerY`, `sizeX`, `sizeY`, `alpha`).
///
/// Measured on Camera Raw renders: the centre is in the unrotated frame like spots;
/// `width` and `height` are semi-axes as fractions of that frame's width and height;
/// `alpha` is the correlation of x and y over the ellipse (|alpha| < 1), which tilts
/// it. `adaptivePupilColor = 1` marks Pet Eye, with a catchlight at `highlightX`,
/// `highlightY` when `showPetEyeHighlight = 1`. `density`, `strength` and `redBias`
/// record Lightroom's detection and did not change the render.
fn red_eye_info(info: &Node, frame: &Frame, skipped: &mut Vec<String>) -> Vec<RedEyeOp> {
    let mut ops = Vec::new();
    for (i, item) in info.items().iter().enumerate() {
        let eye = match item {
            Node::Text(text) => key_values(text),
            Node::Record(_) => {
                let pupil = item.get("pupil");
                let ellipse = pupil.and_then(|p| p.get("ellipse"));
                let mut record = BTreeMap::new();
                for (to, node, from) in [
                    ("x", ellipse, "centerX"),
                    ("y", ellipse, "centerY"),
                    ("width", ellipse, "sizeX"),
                    ("height", ellipse, "sizeY"),
                    ("alpha", ellipse, "alpha"),
                    ("pupilSize", Some(item), "pupilSize"),
                    ("pupilDarkenAmount", Some(item), "pupilDarkenAmount"),
                    ("adaptivePupilColor", Some(item), "adaptivePupilColor"),
                    ("showPetEyeHighlight", Some(item), "showPetEyeHighlight"),
                    ("highlightX", Some(item), "highlightX"),
                    ("highlightY", Some(item), "highlightY"),
                ] {
                    if let Some(v) = node.and_then(|n| n.text(from)) {
                        record.insert(to.to_string(), Node::Text(v.to_string()));
                    }
                }
                Node::Record(record)
            }
            Node::List(_) => Node::Record(BTreeMap::new()),
        };
        match red_eye(&eye, frame) {
            Ok(op) => ops.push(op),
            Err(e) => skipped.push(format!("Red eye {}: {e:#}", i + 1)),
        }
    }
    ops
}
/// `key = value, key = value` as a record.
fn key_values(text: &str) -> Node {
    Node::Record(
        text.split(',')
            .filter_map(|pair| pair.split_once('='))
            .map(|(k, v)| (k.trim().to_string(), Node::Text(v.trim().to_string())))
            .collect(),
    )
}
/// The ellipse's correlation in the unrotated frame.
fn alpha_of(eye: &Node) -> f32 {
    eye.num("alpha").unwrap_or(0.).clamp(
        -crate::model::red_eye::MAX_CORRELATION,
        crate::model::red_eye::MAX_CORRELATION,
    )
}
fn red_eye(eye: &Node, frame: &Frame) -> Result<RedEyeOp> {
    let kind = if eye.flag("adaptivePupilColor") == Some(true) {
        // The catchlight: 0.5 is the centre and 0 or 1 a semi-axis away, in the
        // unrotated frame; Camera Raw leaves out one outside the pupil.
        let catchlight = (eye.flag("showPetEyeHighlight") == Some(true))
            .then(|| {
                [
                    eye.num("highlightX").unwrap_or(0.591),
                    eye.num("highlightY").unwrap_or(0.424),
                ]
            })
            .filter(|h| {
                let offset = h.map(|v| 2. * (v - 0.5));
                crate::model::red_eye::catchlight_inside(offset, alpha_of(eye))
            })
            .map(|h| {
                // Turned with the photo, as positions are.
                let turned = frame.point(h[0], h[1]);
                let center = frame.point(0.5, 0.5);
                [turned[0] - center[0], turned[1] - center[1]].map(|v| 2. * v)
            });
        EyeKind::Pet { catchlight }
    } else {
        EyeKind::Red
    };
    let num = |k: &str| eye.num(k).with_context(|| format!("No {k}"));
    let (x, y) = (num("x")?, num("y")?);
    // Semi-axes as long-edge fractions in the unrotated frame.
    let rx = num("width")? * frame.scale[0];
    let ry = num("height")? * frame.scale[1];
    let alpha = eye.num("alpha").unwrap_or(0.);
    ensure!(alpha.abs() < 1., "Invalid red eye shape");
    // A quarter turn of the camera swaps the axes and mirrors the tilt.
    let (radius, correlation) = if frame.image.turns % 2 == 1 {
        ([ry, rx], -alpha)
    } else {
        ([rx, ry], alpha)
    };
    let op = RedEyeOp {
        kind,
        center: frame.point(x, y),
        radius,
        correlation: correlation.clamp(
            -crate::model::red_eye::MAX_CORRELATION,
            crate::model::red_eye::MAX_CORRELATION,
        ),
        pupil_size: eye
            .num("pupilSize")
            .unwrap_or(crate::model::red_eye::DEFAULT_PUPIL_SIZE)
            .clamp(0., 1.),
        darken: eye
            .num("pupilDarkenAmount")
            .unwrap_or(crate::model::red_eye::DEFAULT_DARKEN)
            .clamp(0., 1.),
    };
    op.validate()?;
    Ok(op)
}
/// A paint mask's dabs as strokes of equal radius (Lightroom changes the radius with
/// "r" entries, e.g. for pen pressure), and the largest radius.
fn dabs(mask: &Node) -> Result<(Vec<Vec<[f32; 2]>>, f32)> {
    let mut radius = mask.num("Radius").context("No brush radius")?;
    let mut strokes: Vec<Vec<[f32; 2]>> = vec![Vec::new()];
    let mut radii = vec![radius];
    let mut largest = radius;
    for d in mask.get("Dabs").map_or(&[][..], Node::items) {
        let Node::Text(t) = d else {
            bail!("Invalid dab");
        };
        let parts: Vec<&str> = t.split_whitespace().collect();
        match parts.as_slice() {
            // Older catalogs write "M" for the same dab positions.
            ["d" | "M", x, y] => strokes.last_mut().unwrap().push([x.parse()?, y.parse()?]),
            ["r", r] => {
                let r: f32 = r.parse()?;
                if (r - radius).abs() > radius * 0.1 && !strokes.last().unwrap().is_empty() {
                    let last = *strokes.last().unwrap().last().unwrap();
                    strokes.push(vec![last]);
                    radii.push(r);
                }
                radius = r;
                *radii.last_mut().unwrap() = r;
                largest = largest.max(r);
            }
            _ => bail!("Invalid dab {t}"),
        }
    }
    strokes.retain(|s| !s.is_empty());
    ensure!(!strokes.is_empty(), "Brush without dabs");
    Ok((strokes, largest))
}
/// One of Lightroom's local corrections as a mask, or `None` when it has no effect.
fn correction(c: &Node, frame: &Frame) -> Result<Option<MaskGroup>> {
    let mut group = MaskGroup {
        name: c
            .text("CorrectionName")
            .unwrap_or("")
            .chars()
            .take(64)
            .collect(),
        amount: c.num("CorrectionAmount").unwrap_or(1.).clamp(0., 2.),
        hidden: c.flag("CorrectionActive") == Some(false),
        adjust: adjustments(c)?,
        ..Default::default()
    };
    for mask in c.get("CorrectionMasks").map_or(&[][..], Node::items) {
        if mask.flag("MaskActive") == Some(false) {
            continue;
        }
        let op = match mask.num("MaskBlendMode").unwrap_or(0.) as i32 {
            0 => MaskOp::Add,
            1 => MaskOp::Subtract,
            2 => MaskOp::Intersect,
            other => bail!("unknown mask blend mode {other}"),
        };
        let (shape, outside) = mask_shape(mask, frame)?;
        let invert = mask.flag("MaskInverted").unwrap_or(false) != outside;
        // Consecutive brush strokes with the same combination form one brush.
        if let (MaskShape::Brush { strokes }, Some(last)) = (&shape, group.components.last_mut())
            && last.op == op
            && last.invert == invert
            && let MaskShape::Brush { strokes: existing } = &mut last.shape
        {
            existing.extend(strokes.iter().cloned());
            continue;
        }
        group.components.push(MaskComponent {
            op: if group.components.is_empty() {
                MaskOp::Add
            } else {
                op
            },
            invert,
            opacity: 1.,
            shape,
        });
    }
    // Lightroom's range mask on a whole correction (before Lightroom Classic 11).
    if let Some(range) = c.get("CorrectionRangeMask") {
        match range.num("Type").unwrap_or(0.) as i32 {
            0 => {}
            2 => {
                let feather = range.num("LumFeather").unwrap_or(0.5).clamp(0., 1.) * 0.2;
                group.components.push(MaskComponent {
                    op: MaskOp::Intersect,
                    ..MaskComponent::new(MaskShape::LuminanceRange {
                        low: range.num("LumMin").unwrap_or(0.).clamp(0., 1.),
                        high: range.num("LumMax").unwrap_or(1.).clamp(0., 1.),
                        falloff: [feather; 2],
                    })
                });
            }
            1 => bail!("Color range masks are not imported"),
            _ => bail!("Depth range masks need AI (not supported)"),
        }
    }
    if group.components.is_empty() {
        return Ok(None);
    }
    group.validate()?;
    Ok(Some(group))
}
/// A mask component's shape, and whether it applies outside that shape (a radial
/// gradient that is not `Flipped`, as Lightroom's Radial Filter does by default).
fn mask_shape(mask: &Node, frame: &Frame) -> Result<(MaskShape, bool)> {
    let pos = |x: &str, y: &str| -> Result<[f32; 2]> {
        Ok(frame.point(
            mask.num(x).with_context(|| format!("No {x}"))?,
            mask.num(y).with_context(|| format!("No {y}"))?,
        ))
    };
    Ok((
        match mask.text("What").unwrap_or("") {
            "Mask/Paint" => {
                let (strokes, _) = dabs(mask)?;
                let value = mask.num("MaskValue").unwrap_or(1.).clamp(0., 1.);
                let feather = (1. - mask.num("CenterWeight").unwrap_or(0.5)).clamp(0., 1.);
                let flow = mask.num("Flow").unwrap_or(1.).clamp(0., 1.);
                let base = mask.num("Radius").context("No brush radius")?;
                MaskShape::Brush {
                    strokes: strokes
                        .into_iter()
                        .map(|points| BrushStroke {
                            points: points.iter().map(|p| frame.point(p[0], p[1])).collect(),
                            radius: base.clamp(1e-4, 0.5),
                            feather,
                            flow,
                            // Lightroom's erase strokes paint a mask value of zero.
                            density: if value > 0. { value } else { 1. },
                            erase: value == 0.,
                            auto_mask: false,
                        })
                        .collect(),
                }
            }
            "Mask/Gradient" => MaskShape::Linear {
                from: pos("FullX", "FullY")?,
                to: pos("ZeroX", "ZeroY")?,
            },
            "Mask/CircularGradient" => {
                let (top, left) = (
                    mask.num("Top").context("No Top")?,
                    mask.num("Left").context("No Left")?,
                );
                let (bottom, right) = (
                    mask.num("Bottom").context("No Bottom")?,
                    mask.num("Right").context("No Right")?,
                );
                if mask.num("Roundness").unwrap_or(0.) != 0.
                    || mask.num("Midpoint").unwrap_or(50.) != 50.
                {
                    bail!(
                        "radial Roundness and Midpoint other than the defaults are not supported"
                    );
                }
                // Radii in long-edge units; Lightroom's angle turns the other way from
                // RAWmakase's, and quarter turns of the camera orientation add to it.
                let radii = [
                    ((right - left) * 0.5 * frame.scale[0])
                        .abs()
                        .clamp(1e-4, 4.),
                    ((bottom - top) * 0.5 * frame.scale[1])
                        .abs()
                        .clamp(1e-4, 4.),
                ];
                let angle = -mask.num("Angle").unwrap_or(0.) + 90. * frame.image.turns as f32;
                let flipped = mask.flag("Flipped").unwrap_or(false);
                let shape = MaskShape::Radial {
                    center: frame.point((left + right) * 0.5, (top + bottom) * 0.5),
                    radii,
                    angle: (angle + 180.).rem_euclid(360.) - 180.,
                    feather: (mask.num("Feather").unwrap_or(50.) / 100.).clamp(0., 1.),
                };
                return Ok((shape, !flipped));
            }
            "Mask/RangeMask" => {
                let range = mask.get("CorrectionRangeMask").unwrap_or(mask);
                match range.num("Type").unwrap_or(0.) as i32 {
                    2 => {
                        let (low, high, falloff) = match range.text("LumRange") {
                            Some(r) => {
                                let v: Vec<f32> = r
                                    .split_whitespace()
                                    .filter_map(|s| s.parse().ok())
                                    .collect();
                                ensure!(v.len() == 4, "invalid luminance range");
                                (v[1], v[2], [v[1] - v[0], v[3] - v[2]])
                            }
                            None => {
                                let f = range.num("LumFeather").unwrap_or(0.5).clamp(0., 1.) * 0.2;
                                (
                                    range.num("LumMin").unwrap_or(0.),
                                    range.num("LumMax").unwrap_or(1.),
                                    [f, f],
                                )
                            }
                        };
                        MaskShape::LuminanceRange {
                            low: low.clamp(0., 1.),
                            high: high.clamp(0., 1.).max(low.clamp(0., 1.)),
                            falloff: falloff.map(|f| f.clamp(0., 1.)),
                        }
                    }
                    1 => bail!("Color range masks are not imported"),
                    _ => bail!("Depth range masks need AI (not supported)"),
                }
            }
            "Mask/Image" => bail!("{} masks need AI (not supported)", ai_kind(mask)),
            other => bail!("unsupported mask {other}"),
        },
        false,
    ))
}
/// Lightroom's AI mask kinds by `MaskSubType`.
fn ai_kind(mask: &Node) -> &'static str {
    match mask.num("MaskSubType").map(|v| v as i32) {
        Some(1) => "Select Subject",
        Some(2) => "Select Sky",
        Some(3) => "People",
        Some(4) => "Object",
        Some(6) => "Background",
        _ => "AI",
    }
}
/// A correction's sliders (Lightroom's XMP values are the sliders divided by 100,
/// Exposure in EV).
fn adjustments(c: &Node) -> Result<LocalAdjust> {
    let v = |key: &str| c.num(key).unwrap_or(0.);
    let unit = |key: &str| v(key).clamp(-1., 1.);
    for key in [
        "LocalMoire",
        "LocalDefringe",
        "LocalGrain",
        "LocalExposure",
        "LocalBrightness",
        "LocalContrast",
        "LocalClarity",
    ] {
        if v(key) != 0. {
            bail!("{key} is not supported");
        }
    }
    for key in ["MainCurve", "RedCurve", "GreenCurve", "BlueCurve"] {
        if let Some(curve) = c.get(key)
            && !is_identity_curve(curve)
        {
            bail!("local tone curves are not supported");
        }
    }
    Ok(LocalAdjust {
        temperature: unit("LocalTemperature"),
        tint: unit("LocalTint"),
        // Normalised to ±1 for ±4 EV, as Camera Raw renders it.
        exposure: (v("LocalExposure2012") * 4.).clamp(-4., 4.),
        contrast: unit("LocalContrast2012"),
        highlights: unit("LocalHighlights2012"),
        shadows: unit("LocalShadows2012"),
        whites: unit("LocalWhites2012"),
        blacks: unit("LocalBlacks2012"),
        texture: unit("LocalTexture"),
        clarity: unit("LocalClarity2012"),
        dehaze: unit("LocalDehaze"),
        hue: (v("LocalHue") * 180.).clamp(-180., 180.),
        saturation: unit("LocalSaturation"),
        sharpness: unit("LocalSharpness"),
        noise: unit("LocalLuminanceNoise"),
        color: [
            (v("LocalToningHue") / 360.).rem_euclid(1.),
            v("LocalToningSaturation").clamp(0., 1.),
        ],
    })
}
fn is_identity_curve(curve: &Node) -> bool {
    let values: Vec<f32> = curve
        .items()
        .iter()
        .filter_map(|n| match n {
            Node::Text(t) => Some(t),
            _ => None,
        })
        .flat_map(|t| t.split(','))
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    values.is_empty() || values == [0., 0., 255., 255.]
}
