//! Typed access to entity proxy-graphics metafiles.
//!
//! Unknown records stay opaque so decoding and re-encoding a metafile does not
//! discard primitives or traits that this module does not model yet.

use crate::types::Vector3;

const HEADER_SIZE: usize = 8;
const RECORD_HEADER_SIZE: usize = 8;
const UNICODE_TEXT_FIXED_SIZE: usize = 96;

/// An entity proxy-graphics metafile.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProxyGraphics {
    pub records: Vec<ProxyGraphicRecord>,
}

/// A typed proxy-graphics record.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ProxyGraphicRecord {
    /// Type 21: an empty record that disables filling for later primitives.
    FillOff,
    /// Type 36: a single-line UTF-16 text primitive.
    UnicodeText(ProxyUnicodeText),
    /// A record whose payload is not interpreted by this version of acadrust.
    Unknown { record_type: u32, data: Vec<u8> },
}

/// Type-36 Unicode text data.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProxyUnicodeText {
    pub position: Vector3,
    pub normal: Vector3,
    pub direction: Vector3,
    pub height: f64,
    pub width_factor: f64,
    pub oblique_angle: f64,
    pub text: String,
}

impl ProxyGraphics {
    /// Decode a complete proxy-graphics metafile.
    pub fn decode(data: &[u8]) -> Option<Self> {
        let total_size = read_u32(data, 0)? as usize;
        let record_count = read_u32(data, 4)? as usize;
        if total_size < HEADER_SIZE || total_size > data.len() {
            return None;
        }

        let mut records = Vec::with_capacity(record_count.min(1_000_000));
        let mut offset = HEADER_SIZE;
        for _ in 0..record_count {
            let record_size = read_u32(data, offset)? as usize;
            let record_type = read_u32(data, offset + 4)?;
            if record_size < RECORD_HEADER_SIZE {
                return None;
            }
            let record_end = offset.checked_add(record_size)?;
            if record_end > total_size {
                return None;
            }
            let payload = &data[offset + RECORD_HEADER_SIZE..record_end];
            records.push(decode_record(record_type, payload));
            offset = record_end;
        }
        if offset != total_size {
            return None;
        }
        Some(Self { records })
    }

    /// Encode the records as a complete proxy-graphics metafile.
    pub fn encode(&self) -> Option<Vec<u8>> {
        let record_count = u32::try_from(self.records.len()).ok()?;
        let mut output = vec![0u8; HEADER_SIZE];
        output[4..8].copy_from_slice(&record_count.to_le_bytes());
        for record in &self.records {
            let (record_type, payload) = encode_record(record)?;
            let record_size = u32::try_from(RECORD_HEADER_SIZE.checked_add(payload.len())?).ok()?;
            output.extend_from_slice(&record_size.to_le_bytes());
            output.extend_from_slice(&record_type.to_le_bytes());
            output.extend_from_slice(&payload);
        }
        let total_size = u32::try_from(output.len()).ok()?;
        output[0..4].copy_from_slice(&total_size.to_le_bytes());
        Some(output)
    }
}

impl super::EntityCommon {
    /// Decode this entity's cached proxy graphics.
    pub fn proxy_graphics(&self) -> Option<ProxyGraphics> {
        ProxyGraphics::decode(self.graphic_data.as_deref()?)
    }

    /// Replace this entity's cached proxy graphics.
    pub fn set_proxy_graphics(&mut self, graphics: &ProxyGraphics) -> bool {
        let Some(data) = graphics.encode() else {
            return false;
        };
        self.graphic_data = Some(data);
        true
    }
}

/// A drawable primitive decoded from proxy graphics (WCS coordinates).
///
/// Produced by [`ProxyGraphics::primitives`]. Only the record kinds a viewer
/// needs to draw AEC/Civil 3D style markers and labels are covered (circle,
/// arc, polyline/polygon, Unicode text, colour index); everything else is
/// skipped without touching the original records, so round-trip is unaffected.
#[derive(Debug, Clone, PartialEq)]
pub enum ProxyPrimitive {
    /// Open or closed polyline. Circles and arcs are tessellated into this.
    Polyline {
        points: Vec<Vector3>,
        closed: bool,
        /// Colour active for this primitive (`None` = entity colour).
        color: Option<(u8, u8, u8)>,
    },
    /// Single-line Unicode text.
    Text {
        text: ProxyUnicodeText,
        color: Option<(u8, u8, u8)>,
    },
}

impl ProxyGraphics {
    /// Short diagnostic summary: `n=<count> [type:payload-bytes ...]`.
    /// Colour-record payloads (14, 22) are shown in hex.
    pub fn describe(&self) -> String {
        let mut s = format!("n={} [", self.records.len());
        for (i, r) in self.records.iter().take(40).enumerate() {
            if i > 0 {
                s.push(' ');
            }
            match r {
                ProxyGraphicRecord::FillOff => s.push_str("21:0"),
                ProxyGraphicRecord::UnicodeText(t) => {
                    s.push_str(&format!("36:txt='{}' h={:.3}", t.text, t.height))
                }
                ProxyGraphicRecord::Unknown { record_type, data } => {
                    if *record_type == 38 {
                        match decode_unicode_text2(data) {
                            Some(t) => s.push_str(&format!(
                                "38:txt='{}' h={:.3} w={:.2}",
                                t.text, t.height, t.width_factor
                            )),
                            None => s.push_str(&format!("38:{}b(cozulemedi)", data.len())),
                        }
                    } else if *record_type == 14 || *record_type == 22 {
                        let hex: String = data.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                        s.push_str(&format!("{}:{}b={}", record_type, data.len(), hex));
                    } else {
                        s.push_str(&format!("{}:{}b", record_type, data.len()));
                    }
                }
            }
        }
        s.push(']');
        s
    }

    /// Walk the records and return drawable primitives, tracking the
    /// "attribute colour" record (type 14) as it changes.
    pub fn primitives(&self) -> Vec<ProxyPrimitive> {
        let mut out = Vec::new();
        let mut color: Option<(u8, u8, u8)> = None;
        for record in &self.records {
            match record {
                ProxyGraphicRecord::FillOff => {}
                ProxyGraphicRecord::UnicodeText(t) => out.push(ProxyPrimitive::Text {
                    text: t.clone(),
                    color,
                }),
                ProxyGraphicRecord::Unknown { record_type, data } => match *record_type {
                    14 => {
                        color = read_i32(data, 0).and_then(|v| {
                            if (1..=255).contains(&v) {
                                crate::types::Color::Index(v as u8).rgb()
                            } else {
                                None // 0 = ByBlock, 256 = ByLayer -> entity colour
                            }
                        });
                    }
                    22 => {
                        // True colour. Layout assumed 0xMMRRGGBB (M = method byte,
                        // 0xC2 = by colour). Unverified against real files.
                        color = read_u32(data, 0).and_then(|v| {
                            if (v >> 24) == 0xC2 {
                                Some((((v >> 16) & 0xff) as u8, ((v >> 8) & 0xff) as u8, (v & 0xff) as u8))
                            } else {
                                None
                            }
                        });
                    }
                    38 => {
                        if let Some(t) = decode_unicode_text2(data) {
                            out.push(ProxyPrimitive::Text { text: t, color });
                        }
                    }
                    10 | 11 => {
                        if let Some(t) = decode_ascii_text(data) {
                            out.push(ProxyPrimitive::Text { text: t, color });
                        }
                    }
                    2 => {
                        if let Some(points) = decode_circle(data) {
                            out.push(ProxyPrimitive::Polyline { points, closed: false, color });
                        }
                    }
                    4 => {
                        if let Some(points) = decode_arc(data) {
                            out.push(ProxyPrimitive::Polyline { points, closed: false, color });
                        }
                    }
                    6 | 7 => {
                        if let Some(points) = decode_point_list(data) {
                            out.push(ProxyPrimitive::Polyline {
                                points,
                                closed: *record_type == 7,
                                color,
                            });
                        }
                    }
                    _ => {}
                },
            }
        }
        out
    }
}

type V3 = (f64, f64, f64);

fn v_cross(a: V3, b: V3) -> V3 {
    (
        a.1 * b.2 - a.2 * b.1,
        a.2 * b.0 - a.0 * b.2,
        a.0 * b.1 - a.1 * b.0,
    )
}

fn v_norm(a: V3) -> Option<V3> {
    let l = (a.0 * a.0 + a.1 * a.1 + a.2 * a.2).sqrt();
    if l < 1e-12 || !l.is_finite() {
        None
    } else {
        Some((a.0 / l, a.1 / l, a.2 / l))
    }
}

/// DXF "arbitrary axis" algorithm -> (x axis, y axis) of the plane.
fn plane_axes(normal: V3) -> Option<(V3, V3)> {
    let n = v_norm(normal)?;
    let ax = if n.0.abs() < 1.0 / 64.0 && n.1.abs() < 1.0 / 64.0 {
        v_cross((0.0, 1.0, 0.0), n)
    } else {
        v_cross((0.0, 0.0, 1.0), n)
    };
    let ax = v_norm(ax)?;
    let ay = v_cross(n, ax);
    Some((ax, ay))
}

fn arc_points(center: V3, radius: f64, ux: V3, uy: V3, sweep: f64) -> Vec<Vector3> {
    let segments = ((sweep.abs() / std::f64::consts::TAU) * 48.0).ceil().max(2.0) as usize;
    let mut pts = Vec::with_capacity(segments + 1);
    for i in 0..=segments {
        let t = sweep * (i as f64 / segments as f64);
        let (s, c) = t.sin_cos();
        pts.push(Vector3::new(
            center.0 + radius * (c * ux.0 + s * uy.0),
            center.1 + radius * (c * ux.1 + s * uy.1),
            center.2 + radius * (c * ux.2 + s * uy.2),
        ));
    }
    pts
}

/// Type 2: center(3d) radius(d) normal(3d).
fn decode_circle(data: &[u8]) -> Option<Vec<Vector3>> {
    let c = read_vector(data, 0)?;
    let radius = read_f64(data, 24)?;
    let n = read_vector(data, 32)?;
    if !(radius.is_finite() && radius > 0.0) {
        return None;
    }
    let (ax, ay) = plane_axes((n.x, n.y, n.z))?;
    Some(arc_points((c.x, c.y, c.z), radius, ax, ay, std::f64::consts::TAU))
}

/// Type 4: center(3d) radius(d) normal(3d) start-vector(3d) sweep(d) [type(i32)].
fn decode_arc(data: &[u8]) -> Option<Vec<Vector3>> {
    let c = read_vector(data, 0)?;
    let radius = read_f64(data, 24)?;
    let n = read_vector(data, 32)?;
    let sv = read_vector(data, 56)?;
    let sweep = read_f64(data, 80)?;
    if !(radius.is_finite() && radius > 0.0 && sweep.is_finite()) {
        return None;
    }
    let nn = v_norm((n.x, n.y, n.z))?;
    let ux = v_norm((sv.x, sv.y, sv.z))?;
    let uy = v_cross(nn, ux);
    Some(arc_points((c.x, c.y, c.z), radius, ux, uy, sweep))
}

/// Types 6 / 7: count(u32) followed by count x 3d points.
fn decode_point_list(data: &[u8]) -> Option<Vec<Vector3>> {
    let count = read_u32(data, 0)? as usize;
    let max = data.len().saturating_sub(4) / 24;
    let count = count.min(max);
    if count < 2 {
        return None;
    }
    let mut pts = Vec::with_capacity(count);
    for i in 0..count {
        pts.push(read_vector(data, 4 + i * 24)?);
    }
    Some(pts)
}

/// Type 38 (Unicode text 2): position, normal, direction (3 x 3d), then a
/// NUL-terminated UTF-16 string, then a block that contains
/// height(d) width_factor(d) oblique(d) ... The block normally starts 8 bytes
/// after the string (two i32 fields), but the offset is probed (0..24) so
/// small layout differences do not break decoding.
fn decode_unicode_text2(data: &[u8]) -> Option<ProxyUnicodeText> {
    let position = read_vector(data, 0)?;
    let normal = read_vector(data, 24)?;
    let direction = read_vector(data, 48)?;
    let start = 72usize;
    let tail = data.get(start..)?;
    let mut units: Vec<u16> = Vec::new();
    let mut i = 0usize;
    loop {
        let b = tail.get(i..i + 2)?;
        let u = u16::from_le_bytes([b[0], b[1]]);
        i += 2;
        if u == 0 {
            break;
        }
        units.push(u);
        if units.len() > 4096 {
            return None;
        }
    }
    let text = String::from_utf16(&units).ok()?;
    let after = start + i;
    for delta in 0..24usize {
        let off = after + delta;
        let (Some(height), Some(width_factor), Some(oblique)) =
            (read_f64(data, off), read_f64(data, off + 8), read_f64(data, off + 16))
        else {
            break;
        };
        if height.is_finite()
            && height > 1e-6
            && height < 1e6
            && width_factor.is_finite()
            && width_factor > 0.05
            && width_factor < 20.0
            && oblique.is_finite()
            && oblique.abs() <= 6.5
        {
            return Some(ProxyUnicodeText {
                position,
                normal,
                direction,
                height,
                width_factor,
                oblique_angle: oblique,
                text,
            });
        }
    }
    None
}

/// Types 10 / 11: position, normal, direction (3 x 3d), NUL-terminated ANSI
/// string, then (after optional 0..3 padding bytes) length(i32) raw(i32)
/// height(d) width_factor(d) oblique(d). The padding is probed because the
/// exact alignment differs between writers.
fn decode_ascii_text(data: &[u8]) -> Option<ProxyUnicodeText> {
    let position = read_vector(data, 0)?;
    let normal = read_vector(data, 24)?;
    let direction = read_vector(data, 48)?;
    let start = 72usize;
    let rel = data.get(start..)?.iter().position(|b| *b == 0)?;
    let text: String = data[start..start + rel].iter().map(|b| *b as char).collect();
    let after = start + rel + 1;
    for pad in 0..4usize {
        let off = after + pad;
        let (Some(height), Some(width_factor), Some(oblique_angle)) =
            (read_f64(data, off + 8), read_f64(data, off + 16), read_f64(data, off + 24))
        else {
            continue;
        };
        if height.is_finite()
            && height > 1e-9
            && height < 1e7
            && width_factor.is_finite()
            && width_factor > 0.0
            && width_factor <= 100.0
        {
            return Some(ProxyUnicodeText {
                position,
                normal,
                direction,
                height,
                width_factor,
                oblique_angle: if oblique_angle.is_finite() { oblique_angle } else { 0.0 },
                text,
            });
        }
    }
    None
}

fn read_i32(data: &[u8], offset: usize) -> Option<i32> {
    let value = data.get(offset..offset + 4)?;
    Some(i32::from_le_bytes(value.try_into().ok()?))
}

fn decode_record(record_type: u32, payload: &[u8]) -> ProxyGraphicRecord {
    match record_type {
        21 if payload.is_empty() => ProxyGraphicRecord::FillOff,
        36 => decode_unicode_text(payload)
            .map(ProxyGraphicRecord::UnicodeText)
            .unwrap_or_else(|| ProxyGraphicRecord::Unknown {
                record_type,
                data: payload.to_vec(),
            }),
        _ => ProxyGraphicRecord::Unknown {
            record_type,
            data: payload.to_vec(),
        },
    }
}

fn encode_record(record: &ProxyGraphicRecord) -> Option<(u32, Vec<u8>)> {
    match record {
        ProxyGraphicRecord::FillOff => Some((21, Vec::new())),
        ProxyGraphicRecord::UnicodeText(text) => {
            let mut data = Vec::with_capacity(UNICODE_TEXT_FIXED_SIZE + text.text.len() * 2 + 4);
            write_vector(&mut data, text.position);
            write_vector(&mut data, text.normal);
            write_vector(&mut data, text.direction);
            data.extend_from_slice(&text.height.to_le_bytes());
            data.extend_from_slice(&text.width_factor.to_le_bytes());
            data.extend_from_slice(&text.oblique_angle.to_le_bytes());
            for value in text.text.encode_utf16() {
                data.extend_from_slice(&value.to_le_bytes());
            }
            data.extend_from_slice(&0u16.to_le_bytes());
            while (RECORD_HEADER_SIZE + data.len()) % 4 != 0 {
                data.push(0);
            }
            Some((36, data))
        }
        ProxyGraphicRecord::Unknown { record_type, data } => Some((*record_type, data.clone())),
    }
}

fn decode_unicode_text(data: &[u8]) -> Option<ProxyUnicodeText> {
    if data.len() < UNICODE_TEXT_FIXED_SIZE + 2 {
        return None;
    }
    let position = read_vector(data, 0)?;
    let normal = read_vector(data, 24)?;
    let direction = read_vector(data, 48)?;
    let height = read_f64(data, 72)?;
    let width_factor = read_f64(data, 80)?;
    let oblique_angle = read_f64(data, 88)?;
    let tail = &data[UNICODE_TEXT_FIXED_SIZE..];
    let terminator = tail.chunks_exact(2).position(|value| value == [0, 0])?;
    let units = tail[..terminator * 2]
        .chunks_exact(2)
        .map(|value| u16::from_le_bytes([value[0], value[1]]))
        .collect::<Vec<_>>();
    let text = String::from_utf16(&units).ok()?;
    Some(ProxyUnicodeText {
        position,
        normal,
        direction,
        height,
        width_factor,
        oblique_angle,
        text,
    })
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    let value = data.get(offset..offset + 4)?;
    Some(u32::from_le_bytes(value.try_into().ok()?))
}

fn read_f64(data: &[u8], offset: usize) -> Option<f64> {
    let value = data.get(offset..offset + 8)?;
    Some(f64::from_le_bytes(value.try_into().ok()?))
}

fn read_vector(data: &[u8], offset: usize) -> Option<Vector3> {
    Some(Vector3::new(
        read_f64(data, offset)?,
        read_f64(data, offset + 8)?,
        read_f64(data, offset + 16)?,
    ))
}

fn write_vector(output: &mut Vec<u8>, value: Vector3) {
    output.extend_from_slice(&value.x.to_le_bytes());
    output.extend_from_slice(&value.y.to_le_bytes());
    output.extend_from_slice(&value.z.to_le_bytes());
}
