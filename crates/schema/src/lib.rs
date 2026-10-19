//! Schema definition: tables and fields, loadable from JSON.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid schema: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Id,
    Text,
    Int,
    Float,
    Bool,
    Date,
    Time,
    Enum,
    /// List of image file paths (relative to the data file).
    Images,
    /// Path of a film image (relative to the data file); only in the enlargement table.
    #[serde(rename = "image_ref")]
    ImageRef,
}

/// What a table is for, when the app needs to know (image linking).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TableRole {
    /// Holds films whose `images` can be enlarged.
    Film,
    /// Holds enlargements, each pointing at a film image via an `image_ref` field.
    Enlargement,
    /// Holds per-image notes (tags, location), one row per image via an `image_ref` field.
    #[serde(rename = "image_info")]
    ImageInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: FieldType,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub readonly: bool,
    #[serde(default)]
    pub multiline: bool,
    /// Text format: dates use chrono syntax (`%d.%m.%Y`), times `%M:%S` or `%H:%M`.
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub decimals: Option<usize>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default = "yes")]
    pub visible_in_table: bool,
}

fn yes() -> bool {
    true
}

impl Field {
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
    pub fn date_format(&self) -> &str {
        self.format.as_deref().unwrap_or("%d.%m.%Y")
    }
    pub fn time_format(&self) -> &str {
        self.format.as_deref().unwrap_or("%M:%S")
    }
}

/// One label/value pair on a printed card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardCell {
    pub field: String,
    #[serde(default)]
    pub label: Option<String>,
    /// Left edge of the label from the card's left edge, in mm (default 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<f32>,
    /// Left edge of the value, in mm (default: the card's `label_width`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_at: Option<f32>,
}

/// Wrapped free text below the cell grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardText {
    pub field: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// Print layout of one row as a ruled card (mm, A4 portrait, origin top left).
/// Rows of `cells` are 5 mm bands; the card is `heights[value of height_field]` tall.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    #[serde(default = "d_x")]
    pub x: f32,
    #[serde(default = "d_y")]
    pub y: f32,
    #[serde(default = "d_width")]
    pub width: f32,
    #[serde(default = "d_y")]
    pub bottom_margin: f32,
    /// Distance of the vertical divider after the label column.
    #[serde(default = "d_label_width")]
    pub label_width: f32,
    /// Enum field whose value picks the height; `heights["default"]` is the fallback.
    pub height_field: Option<String>,
    pub heights: std::collections::BTreeMap<String, f32>,
    pub cells: Vec<Vec<CardCell>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<CardText>,
}

fn d_x() -> f32 {
    40.0
}
fn d_y() -> f32 {
    20.0
}
fn d_width() -> f32 {
    130.0
}
fn d_label_width() -> f32 {
    25.0
}

impl Card {
    /// Card height in mm for the value of `height_field` (`None` when it is empty or unlisted).
    pub fn height_for(&self, key: Option<&str>) -> f32 {
        key.and_then(|k| self.heights.get(k))
            .or_else(|| self.heights.get("default"))
            .or_else(|| self.heights.values().next())
            .copied()
            .unwrap_or(5.0 * self.cells.len() as f32)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<TableRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<Card>,
    pub fields: Vec<Field>,
}

impl Table {
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
    pub fn field_index(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.name == name)
    }
    /// Index of the first `image_ref` field.
    pub fn image_ref_index(&self) -> Option<usize> {
        self.fields.iter().position(|f| f.ty == FieldType::ImageRef)
    }
    /// Index of the first `images` field.
    pub fn images_index(&self) -> Option<usize> {
        self.fields.iter().position(|f| f.ty == FieldType::Images)
    }
    /// Index of the `id` field, if the table has one.
    pub fn id_index(&self) -> Option<usize> {
        self.fields.iter().position(|f| f.ty == FieldType::Id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    pub name: String,
    #[serde(default = "one", alias = "version")]
    pub schema_version: u32,
    pub tables: Vec<Table>,
}

fn one() -> u32 {
    1
}

impl Schema {
    pub fn from_json(s: &str) -> Result<Schema, SchemaError> {
        let schema: Schema = serde_json::from_str(s)?;
        schema.validate()?;
        Ok(schema)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("schema serializes")
    }

    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.name == name)
    }

    /// Add image-link roles and the `source_image` field to a schema made before they existed.
    /// Only touches the default table names; returns whether anything changed.
    pub fn upgrade_links(&mut self) -> bool {
        if self.tables.iter().any(|t| t.role.is_some()) {
            return false;
        }
        let has = |s: &Schema, n: &str| s.table(n).is_some_and(|t| t.images_index().is_some());
        if !has(self, "enlargement") || !(has(self, "roll_film") || has(self, "plan_film")) {
            return false;
        }
        for t in &mut self.tables {
            t.role = match t.name.as_str() {
                "roll_film" | "plan_film" if t.images_index().is_some() => Some(TableRole::Film),
                "enlargement" => Some(TableRole::Enlargement),
                _ => None,
            };
            if t.role == Some(TableRole::Enlargement) && t.image_ref_index().is_none() {
                let at = t.images_index().unwrap_or(t.fields.len());
                let field = Field {
                    name: "source_image".into(),
                    ty: FieldType::ImageRef,
                    label: Some("Quellbild".into()),
                    required: false,
                    readonly: false,
                    multiline: false,
                    format: None,
                    default: None,
                    min: None,
                    max: None,
                    decimals: None,
                    unit: None,
                    options: vec![],
                    visible_in_table: true,
                };
                t.fields.insert(at, field);
            }
        }
        true
    }

    /// The per-image notes table (tags, location) that older schemas lack.
    pub fn image_info_table() -> Table {
        let text = |name: &str, label: &str| Field {
            name: name.into(),
            ty: FieldType::Text,
            label: Some(label.into()),
            required: false,
            readonly: false,
            multiline: false,
            format: None,
            default: None,
            min: None,
            max: None,
            decimals: None,
            unit: None,
            options: vec![],
            visible_in_table: true,
        };
        let mut source = text("source_image", "Bild");
        source.ty = FieldType::ImageRef;
        Table { name: "image_info".into(), label: Some("Bildinfo".into()), role: Some(TableRole::ImageInfo), card: None, fields: vec![source, text("tags", "Tags"), text("location", "Ort")] }
    }

    /// Append the `image_info` table to a schema that has film tables but no such table.
    /// Returns whether anything changed.
    pub fn upgrade_image_info(&mut self) -> bool {
        if self.role_index(TableRole::ImageInfo).is_some() || self.role_index(TableRole::Film).is_none() || self.table("image_info").is_some() {
            return false;
        }
        self.tables.push(Self::image_info_table());
        true
    }

    /// Index of the first table with the given role.
    pub fn role_index(&self, role: TableRole) -> Option<usize> {
        self.tables.iter().position(|t| t.role == Some(role))
    }

    pub fn validate(&self) -> Result<(), SchemaError> {
        let bad = |m: String| Err(SchemaError::Invalid(m));
        if self.tables.iter().filter(|t| t.role == Some(TableRole::Enlargement)).count() > 1 {
            return bad("more than one enlargement table".into());
        }
        if self.tables.iter().filter(|t| t.role == Some(TableRole::ImageInfo)).count() > 1 {
            return bad("more than one image_info table".into());
        }
        let mut tnames = HashSet::new();
        for t in &self.tables {
            if !tnames.insert(&t.name) {
                return bad(format!("duplicate table '{}'", t.name));
            }
            let mut fnames = HashSet::new();
            let mut ids = 0;
            for f in &t.fields {
                if !fnames.insert(&f.name) {
                    return bad(format!("duplicate field '{}.{}'", t.name, f.name));
                }
                if f.ty == FieldType::Id {
                    ids += 1;
                }
                if f.ty == FieldType::Enum && f.options.is_empty() {
                    return bad(format!("enum '{}.{}' has no options", t.name, f.name));
                }
                if let (Some(a), Some(b)) = (f.min, f.max) {
                    if a > b {
                        return bad(format!("'{}.{}': min > max", t.name, f.name));
                    }
                }
            }
            let refs = t.fields.iter().filter(|f| f.ty == FieldType::ImageRef).count();
            if refs > 0 && !matches!(t.role, Some(TableRole::Enlargement | TableRole::ImageInfo)) {
                return bad(format!("image_ref in table '{}' which is not an enlargement or image_info table", t.name));
            }
            if matches!(t.role, Some(TableRole::Enlargement | TableRole::ImageInfo)) && refs != 1 {
                return bad(format!("table '{}' needs exactly one image_ref field", t.name));
            }
            if t.role == Some(TableRole::Film) && t.images_index().is_none() {
                return bad(format!("film table '{}' has no images field", t.name));
            }
            if ids > 1 {
                return bad(format!("table '{}' has more than one id field", t.name));
            }
            if let Some(c) = &t.card {
                let known = |n: &str| t.field_index(n).is_some();
                let unknown = c
                    .cells
                    .iter()
                    .flatten()
                    .map(|cell| &cell.field)
                    .chain(c.text.iter().map(|x| &x.field))
                    .chain(c.height_field.iter())
                    .find(|n| !known(n));
                if let Some(n) = unknown {
                    return bad(format!("card of '{}' uses unknown field '{n}'", t.name));
                }
                if let Some(hf) = &c.height_field {
                    let f = &t.fields[t.field_index(hf).expect("checked above")];
                    if f.ty != FieldType::Enum {
                        return bad(format!("card height_field '{}.{hf}' is not an enum", t.name));
                    }
                    if let Some(k) = c.heights.keys().find(|k| *k != "default" && !f.options.contains(k)) {
                        return bad(format!("card height '{k}' is not an option of '{}.{hf}'", t.name));
                    }
                }
                if c.heights.is_empty() || c.cells.is_empty() {
                    return bad(format!("card of '{}' needs heights and cells", t.name));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_info_upgrade_is_idempotent() {
        let j = r#"{"name":"x","tables":[{"name":"f","role":"film","fields":[{"name":"images","type":"images"}]}]}"#;
        let mut s = Schema::from_json(j).unwrap();
        assert!(s.upgrade_image_info());
        assert!(!s.upgrade_image_info());
        assert_eq!(s.role_index(TableRole::ImageInfo), Some(1));
        s.validate().unwrap();
        assert_eq!(Schema::from_json(&s.to_json()).unwrap(), s);
    }

    const CARD: &str = r#"{"name":"x","tables":[{"name":"f","role":"film","fields":[
        {"name":"fmt","type":"enum","options":["a","b"]},{"name":"n","type":"text"},{"name":"images","type":"images"}],
        "card":{"height_field":"fmt","heights":{"a":36,"default":58},"cells":[[{"field":"n","label":"N"}]],
        "text":{"field":"n"}}}]}"#;

    #[test]
    fn card_defaults_roundtrip_and_heights() {
        let s = Schema::from_json(CARD).unwrap();
        let c = s.tables[0].card.as_ref().unwrap();
        assert_eq!((c.x, c.y, c.width, c.bottom_margin, c.label_width), (40.0, 20.0, 130.0, 20.0, 25.0));
        assert_eq!(c.height_for(Some("a")), 36.0);
        assert_eq!(c.height_for(Some("b")), 58.0);
        assert_eq!(c.height_for(None), 58.0);
        assert_eq!(Schema::from_json(&s.to_json()).unwrap(), s);
    }

    #[test]
    fn card_rejects_bad_references() {
        for bad in [
            CARD.replace(r#""field":"n","label":"N""#, r#""field":"zzz""#),
            CARD.replace(r#""height_field":"fmt""#, r#""height_field":"n""#),
            CARD.replace(r#""a":36"#, r#""zz":36"#),
            CARD.replace(r#""heights":{"a":36,"default":58}"#, r#""heights":{}"#),
        ] {
            assert!(Schema::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parse_and_roundtrip() {
        let j = r#"{"name":"x","tables":[{"name":"t","fields":[
            {"name":"id","type":"id"},{"name":"f","type":"enum","options":["a"]}]}]}"#;
        let s = Schema::from_json(j).unwrap();
        assert_eq!(s.tables[0].id_index(), Some(0));
        assert_eq!(Schema::from_json(&s.to_json()).unwrap(), s);
    }

    #[test]
    fn roles_and_image_ref() {
        let ok = r#"{"name":"x","tables":[
            {"name":"f","role":"film","fields":[{"name":"images","type":"images"}]},
            {"name":"e","role":"enlargement","fields":[{"name":"src","type":"image_ref"}]}]}"#;
        let s = Schema::from_json(ok).unwrap();
        assert_eq!(s.role_index(TableRole::Enlargement), Some(1));
        assert_eq!(s.tables[1].image_ref_index(), Some(0));
        assert_eq!(Schema::from_json(&s.to_json()).unwrap(), s);
        let stray = r#"{"name":"x","tables":[{"name":"t","fields":[{"name":"src","type":"image_ref"}]}]}"#;
        assert!(Schema::from_json(stray).is_err());
        let none = r#"{"name":"x","tables":[{"name":"e","role":"enlargement","fields":[]}]}"#;
        assert!(Schema::from_json(none).is_err());
        let noimg = r#"{"name":"x","tables":[{"name":"f","role":"film","fields":[]}]}"#;
        assert!(Schema::from_json(noimg).is_err());
    }

    #[test]
    fn upgrades_old_default_schema() {
        let old = r#"{"name":"x","tables":[
            {"name":"roll_film","fields":[{"name":"images","type":"images"}]},
            {"name":"enlargement","fields":[{"name":"d","type":"text"},{"name":"images","type":"images"}]}]}"#;
        let mut s: Schema = serde_json::from_str(old).unwrap();
        assert!(s.upgrade_links());
        s.validate().unwrap();
        assert_eq!(s.tables[1].image_ref_index(), Some(1));
        assert!(!s.upgrade_links());
        let mut other: Schema = serde_json::from_str(r#"{"name":"x","tables":[{"name":"t","fields":[]}]}"#).unwrap();
        assert!(!other.upgrade_links());
    }

    #[test]
    fn rejects_bad() {
        let dup = r#"{"name":"x","tables":[{"name":"t","fields":[
            {"name":"a","type":"text"},{"name":"a","type":"int"}]}]}"#;
        assert!(Schema::from_json(dup).is_err());
        let en = r#"{"name":"x","tables":[{"name":"t","fields":[{"name":"a","type":"enum"}]}]}"#;
        assert!(Schema::from_json(en).is_err());
    }
}
