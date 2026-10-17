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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    pub fields: Vec<Field>,
}

impl Table {
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
    pub fn field_index(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.name == name)
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

    pub fn validate(&self) -> Result<(), SchemaError> {
        let bad = |m: String| Err(SchemaError::Invalid(m));
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
            if ids > 1 {
                return bad(format!("table '{}' has more than one id field", t.name));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_roundtrip() {
        let j = r#"{"name":"x","tables":[{"name":"t","fields":[
            {"name":"id","type":"id"},{"name":"f","type":"enum","options":["a"]}]}]}"#;
        let s = Schema::from_json(j).unwrap();
        assert_eq!(s.tables[0].id_index(), Some(0));
        assert_eq!(Schema::from_json(&s.to_json()).unwrap(), s);
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
