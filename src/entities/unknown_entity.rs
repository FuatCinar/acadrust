//! Unknown entity type for round-trip preservation.
//!
//! When the reader encounters an entity type that is not directly supported,
//! it captures the common entity properties (handle, layer, color, …) and
//! the raw record data so the entity can be written back losslessly.
//!
//! For DWG files, the entire merged-stream record is stored in
//! [`raw_dwg_data`](UnknownEntity::raw_dwg_data) together with the
//! original DWG type code.  The writer emits these bytes verbatim,
//! preserving the entity exactly as it was in the source file.
//!
//! For DXF files, the entity-specific group-code pairs are stored in
//! [`raw_dxf_codes`](UnknownEntity::raw_dxf_codes) so they can be
//! written back alongside the common entity data.

use crate::entities::{Entity, EntityCommon};
use crate::types::{BoundingBox3D, Color, Handle, LineWeight, Transform, Transparency, Vector3};

/// An entity whose type is not directly supported by the library.
///
/// Preserves the DXF/DWG type name and common entity properties.
/// When raw data is available (DWG `raw_dwg_data` or DXF
/// `raw_dxf_codes`), the entity is written back losslessly.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UnknownEntity {
    /// Common entity data (handle, layer, color, reactors, …).
    pub common: EntityCommon,
    /// The DXF type name as it appeared in the file (e.g. `"ACAD_PROXY_ENTITY"`).
    pub dxf_name: String,
    /// DWG object type code (from the binary record header).
    /// `0` if the entity did not come from a DWG file.
    pub dwg_type_code: i16,
    /// Raw DWG merged-stream record bytes.
    ///
    /// This is the exact payload between the ModularShort length prefix
    /// and the CRC-16 trailer.  When present the writer emits these
    /// bytes verbatim (with fresh framing) so the entity survives a
    /// roundtrip without any data loss.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub raw_dwg_data: Option<Vec<u8>>,
    /// Handle-stream bit count for R2010+ DWG records.
    ///
    /// Stored alongside `raw_dwg_data` because R2010+ records require
    /// a ModularChar(handle_bits) field in the framing header.
    pub dwg_handle_bits: i64,
    /// Raw DXF entity-specific group-code pairs.
    ///
    /// Each entry is `(group_code, value_string)`.  When present the
    /// DXF writer emits the common entity header followed by these
    /// pairs, reproducing the original entity content.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub raw_dxf_codes: Option<Vec<(i32, String)>>,
    /// DWG version `raw_dwg_data` was read from (drop on incompatible cross-version save).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub dwg_source_version: Option<crate::types::DxfVersion>,
}

impl UnknownEntity {
    /// Create a new unknown entity with the given DXF type name.
    pub fn new(dxf_name: impl Into<String>) -> Self {
        Self {
            common: EntityCommon::new(),
            dxf_name: dxf_name.into(),
            dwg_type_code: 0,
            raw_dwg_data: None,
            dwg_handle_bits: 0,
            raw_dxf_codes: None,
            dwg_source_version: None,
        }
    }

    /// Proxy graphics of this entity (e.g. Civil 3D `AECC_COGO_POINT`).
    ///
    /// DWG: the graphics blob is stored in `common.graphic_data`.
    /// DXF: it arrives as group-code 310 hex chunks inside `raw_dxf_codes`
    /// (proxy entity data may follow it, so the blob is cut to the size
    /// declared in its own header before decoding).
    pub fn proxy_graphics(&self) -> Option<crate::entities::ProxyGraphics> {
        if let Some(g) = self.common.proxy_graphics() {
            return Some(g);
        }
        let codes = self.raw_dxf_codes.as_ref()?;
        let mut bytes: Vec<u8> = Vec::new();
        for (code, value) in codes {
            if *code != 310 {
                continue;
            }
            let v = value.trim().as_bytes();
            let mut i = 0;
            while i + 1 < v.len() {
                let hi = (v[i] as char).to_digit(16)?;
                let lo = (v[i + 1] as char).to_digit(16)?;
                bytes.push((hi * 16 + lo) as u8);
                i += 2;
            }
        }
        if bytes.len() < 8 {
            return None;
        }
        let total = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        if total < 8 || total > bytes.len() {
            return None;
        }
        crate::entities::ProxyGraphics::decode(&bytes[..total])
    }
}

impl Entity for UnknownEntity {
    fn handle(&self) -> Handle {
        self.common.handle
    }
    fn set_handle(&mut self, handle: Handle) {
        self.common.handle = handle;
    }
    fn layer(&self) -> &str {
        &self.common.layer
    }
    fn set_layer(&mut self, layer: String) {
        self.common.layer = layer;
    }
    fn color(&self) -> Color {
        self.common.color
    }
    fn set_color(&mut self, color: Color) {
        self.common.color = color;
    }
    fn line_weight(&self) -> LineWeight {
        self.common.line_weight
    }
    fn set_line_weight(&mut self, weight: LineWeight) {
        self.common.line_weight = weight;
    }
    fn transparency(&self) -> Transparency {
        self.common.transparency
    }
    fn set_transparency(&mut self, transparency: Transparency) {
        self.common.transparency = transparency;
    }
    fn is_invisible(&self) -> bool {
        self.common.invisible
    }
    fn set_invisible(&mut self, invisible: bool) {
        self.common.invisible = invisible;
    }
    fn bounding_box(&self) -> BoundingBox3D {
        BoundingBox3D::from_point(Vector3::ZERO)
    }
    fn translate(&mut self, _offset: Vector3) {
        super::translate::translate_unknown(self, _offset);
    }
    fn entity_type(&self) -> &'static str {
        "UNKNOWN"
    }
    fn apply_transform(&mut self, _transform: &Transform) {
        super::transform::transform_unknown(self, _transform);
    }
}
