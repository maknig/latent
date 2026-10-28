//! Typed values, rows and tables driven by an `latent-schema` schema, plus CSV/JSON IO.
use chrono::NaiveDate;
use latent_schema::{Field, FieldType, Schema, Table};
use serde_json::{json, Value as Json};
use std::cmp::Ordering;
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum DataError {
    #[error("{0}")]
    Parse(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("csv: {0}")]
    Csv(#[from] csv::Error),
    #[error("schema: {0}")]
    Schema(#[from] latent_schema::SchemaError),
    #[error("unknown table '{0}'")]
    UnknownTable(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Date(NaiveDate),
    /// Duration in seconds (displayed as `mm:ss` or `hh:mm`).
    Time(u32),
}

impl Value {
    /// Total ordering for sorting; Null sorts first, mixed types by variant.
    pub fn cmp_value(&self, o: &Value) -> Ordering {
        use Value::*;
        match (self, o) {
            (Null, Null) => Ordering::Equal,
            (Null, _) => Ordering::Less,
            (_, Null) => Ordering::Greater,
            (Text(a), Text(b)) => a.to_lowercase().cmp(&b.to_lowercase()),
            (Int(a), Int(b)) => a.cmp(b),
            (Float(a), Float(b)) => a.total_cmp(b),
            (Int(a), Float(b)) => (*a as f64).total_cmp(b),
            (Float(a), Int(b)) => a.total_cmp(&(*b as f64)),
            (Bool(a), Bool(b)) => a.cmp(b),
            (Date(a), Date(b)) => a.cmp(b),
            (Time(a), Time(b)) => a.cmp(b),
            _ => Ordering::Equal,
        }
    }
}

/// Sort key of an image path: files with a number at the end of the name (`IMG_0012.jpg` -> 12)
/// come first ordered by that number, then the others; ties are ordered by file name.
fn image_sort_key(path: &str) -> (bool, u128, String) {
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let digits = stem.len() - stem.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let number = stem[stem.len() - digits..].parse::<u128>();
    match number {
        Ok(n) => (false, n, name.to_lowercase()),
        Err(_) if digits > 0 => (false, u128::MAX, name.to_lowercase()),
        Err(_) => (true, 0, name.to_lowercase()),
    }
}

/// Paths held by an `images` cell, sorted by the number at the end of the file name.
pub fn image_paths(v: &Value) -> Vec<String> {
    match v {
        Value::Text(s) => {
            let mut paths: Vec<String> = s
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();
            paths.sort_by_cached_key(|p| image_sort_key(p));
            paths
        }
        _ => vec![],
    }
}

/// Warning text returned by `Store::load` when an old schema got image links added.
pub const UPGRADE_WARNING: &str =
    "schema upgraded: image links and image info added (save to keep)";

/// Film table name and row id encoded in an image path `images/<table>/<id:04>/<file>`.
pub fn film_row_for(path: &str) -> Option<(String, u64)> {
    let mut parts = path.split('/');
    if parts.next()? != "images" {
        return None;
    }
    let table = parts.next()?.to_string();
    let id = parts.next()?.parse().ok()?;
    parts.next()?;
    Some((table, id))
}

/// Ids of the enlargement rows whose `image_ref` is `path`.
pub fn enlargements_for(store: &Store, path: &str) -> Vec<u64> {
    let Some(ti) = store
        .schema
        .role_index(latent_schema::TableRole::Enlargement)
    else {
        return vec![];
    };
    let Some(fi) = store.schema.tables[ti].image_ref_index() else {
        return vec![];
    };
    store.tables[ti]
        .rows
        .iter()
        .filter(|r| matches!(&r.values[fi], Value::Text(s) if s.trim() == path))
        .map(|r| r.id)
        .collect()
}

/// Id of the `image_info` row whose `image_ref` is `path`.
pub fn image_info_for(store: &Store, path: &str) -> Option<u64> {
    let ti = store
        .schema
        .role_index(latent_schema::TableRole::ImageInfo)?;
    let fi = store.schema.tables[ti].image_ref_index()?;
    store.tables[ti]
        .rows
        .iter()
        .find(|r| matches!(&r.values[fi], Value::Text(s) if s.trim() == path))
        .map(|r| r.id)
}

pub fn format_time(secs: u32, fmt: &str) -> String {
    if fmt == "%H:%M" {
        format!("{:02}:{:02}", secs / 3600, (secs / 60) % 60)
    } else {
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }
}

pub fn parse_time(s: &str, fmt: &str) -> Option<u32> {
    let (a, b) = s.split_once(':')?;
    let (a, b): (u32, u32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    if b >= 60 {
        return None;
    }
    Some(if fmt == "%H:%M" {
        a * 3600 + b * 60
    } else {
        a * 60 + b
    })
}

/// Human-readable / CSV text of a value.
pub fn format_value(v: &Value, f: &Field) -> String {
    match v {
        Value::Null => String::new(),
        Value::Text(s) => s.clone(),
        Value::Int(i) => i.to_string(),
        Value::Float(x) => match f.decimals {
            Some(d) => format!("{x:.d$}"),
            None => x.to_string(),
        },
        Value::Bool(b) => b.to_string(),
        Value::Date(d) => d.format(f.date_format()).to_string(),
        Value::Time(t) => format_time(*t, f.time_format()),
    }
}

/// Parse text into a value of the field's type. Empty text gives `Null`.
pub fn parse_value(s: &str, f: &Field) -> Result<Value, DataError> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Value::Null);
    }
    let err = |what: &str| DataError::Parse(format!("field '{}': invalid {what} '{s}'", f.name));
    Ok(match f.ty {
        FieldType::Id | FieldType::Int => Value::Int(s.parse().map_err(|_| err("integer"))?),
        FieldType::Float => Value::Float(s.replace(',', ".").parse().map_err(|_| err("number"))?),
        FieldType::Bool => Value::Bool(matches!(
            s.to_lowercase().as_str(),
            "1" | "true" | "yes" | "ja"
        )),
        FieldType::Date => {
            Value::Date(NaiveDate::parse_from_str(s, f.date_format()).map_err(|_| err("date"))?)
        }
        FieldType::Time => Value::Time(parse_time(s, f.time_format()).ok_or_else(|| err("time"))?),
        FieldType::Text | FieldType::Enum | FieldType::Images | FieldType::ImageRef => {
            Value::Text(s.to_string())
        }
    })
}

/// Default value for a new row.
pub fn default_value(f: &Field) -> Value {
    match (&f.default, f.ty) {
        (Some(Json::String(s)), FieldType::Date) if s == "today" => {
            Value::Date(chrono::Local::now().date_naive())
        }
        (Some(j), _) => from_json(j, f).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

pub fn to_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Text(s) => json!(s),
        Value::Int(i) => json!(i),
        Value::Float(x) => json!(x),
        Value::Bool(b) => json!(b),
        Value::Date(d) => json!(d.format("%Y-%m-%d").to_string()),
        Value::Time(t) => json!(t),
    }
}

pub fn from_json(j: &Json, f: &Field) -> Result<Value, DataError> {
    let bad = || DataError::Parse(format!("field '{}': bad json value {j}", f.name));
    Ok(match (j, f.ty) {
        (Json::Null, _) => Value::Null,
        (Json::String(s), FieldType::Date) => {
            Value::Date(NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| bad())?)
        }
        (Json::String(s), FieldType::Text | FieldType::Enum | FieldType::ImageRef) => {
            Value::Text(s.clone())
        }
        (Json::Number(n), FieldType::Int | FieldType::Id) => {
            Value::Int(n.as_i64().ok_or_else(bad)?)
        }
        (Json::Number(n), FieldType::Float) => Value::Float(n.as_f64().ok_or_else(bad)?),
        (Json::Number(n), FieldType::Time) => Value::Time(n.as_u64().ok_or_else(bad)? as u32),
        (Json::Bool(b), FieldType::Bool) => Value::Bool(*b),
        _ => return Err(bad()),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Stable auto-increment key (also the value of the `id` field, if any).
    pub id: u64,
    pub values: Vec<Value>,
    /// Values for keys the schema does not know (kept until purged).
    pub orphans: BTreeMap<String, Json>,
}

impl Row {
    pub fn new(id: u64, values: Vec<Value>) -> Row {
        Row {
            id,
            values,
            orphans: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableData {
    pub name: String,
    pub next_id: u64,
    pub rows: Vec<Row>,
}

impl TableData {
    pub fn new(name: &str) -> Self {
        TableData {
            name: name.into(),
            next_id: 1,
            rows: vec![],
        }
    }

    /// A blank row with schema defaults (not yet inserted).
    pub fn blank_values(table: &Table) -> Vec<Value> {
        table.fields.iter().map(default_value).collect()
    }

    /// Insert a row, assigning the next id and mirroring it into the `id` field.
    pub fn insert(&mut self, table: &Table, mut values: Vec<Value>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        if let Some(i) = table.id_index() {
            values[i] = Value::Int(id as i64);
        }
        self.rows.push(Row::new(id, values));
        id
    }

    pub fn remove(&mut self, id: u64) -> Option<Row> {
        let p = self.rows.iter().position(|r| r.id == id)?;
        Some(self.rows.remove(p))
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Row> {
        self.rows.iter_mut().find(|r| r.id == id)
    }

    /// CSV without header, comma separated, CRLF, in schema field order.
    pub fn to_csv(&self, table: &Table) -> Result<String, DataError> {
        let mut w = csv::WriterBuilder::new()
            .has_headers(false)
            .terminator(csv::Terminator::CRLF)
            .from_writer(vec![]);
        for r in &self.rows {
            let rec: Vec<String> = table
                .fields
                .iter()
                .zip(&r.values)
                .map(|(f, v)| format_value(v, f))
                .collect();
            w.write_record(&rec)?;
        }
        let bytes = w
            .into_inner()
            .map_err(|e| DataError::Parse(e.to_string()))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Import CSV (no header). Rows get ids from the `id` column when present,
    /// otherwise auto-assigned. Missing trailing columns become Null.
    pub fn from_csv(table: &Table, text: &str) -> Result<TableData, DataError> {
        let text = text.replace("\r\n", "\n");
        let mut rd = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(text.trim_start_matches('\u{feff}').as_bytes());
        let mut td = TableData::new(&table.name);
        for (n, rec) in rd.records().enumerate() {
            let rec = rec?;
            if rec.iter().all(|c| c.trim().is_empty()) {
                continue;
            }
            let mut values = Vec::with_capacity(table.fields.len());
            for (i, f) in table.fields.iter().enumerate() {
                let v = parse_value(rec.get(i).unwrap_or(""), f)
                    .map_err(|e| DataError::Parse(format!("row {}: {e}", n + 1)))?;
                values.push(v);
            }
            match table.id_index().map(|i| &values[i]) {
                Some(Value::Int(id)) if *id > 0 => {
                    let id = *id as u64;
                    td.next_id = td.next_id.max(id + 1);
                    td.rows.push(Row::new(id, values));
                }
                _ => {
                    td.insert(table, values);
                }
            }
        }
        Ok(td)
    }
}

/// All tables of a schema with their data.
#[derive(Debug, Clone, PartialEq)]
pub struct Store {
    pub schema: Schema,
    pub tables: Vec<TableData>,
    /// Data tables the schema does not know; kept and written back.
    pub orphan_tables: BTreeMap<String, Json>,
}

/// Cell encoding in the data file: ISO dates, `mm:ss` times.
fn cell_to_json(v: &Value, f: &Field) -> Json {
    match v {
        Value::Time(t) => json!(format_time(*t, f.time_format())),
        Value::Text(s) if f.ty == FieldType::Images => json!(image_paths(&Value::Text(s.clone()))),
        other => to_json(other),
    }
}

fn cell_from_json(j: &Json, f: &Field) -> Result<Value, DataError> {
    match (j, f.ty) {
        (Json::Array(a), FieldType::Images) => Ok(Value::Text(
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )),
        (Json::String(s), FieldType::Time) => parse_time(s, f.time_format())
            .map(Value::Time)
            .ok_or_else(|| DataError::Parse(format!("field '{}': invalid time '{s}'", f.name))),
        (Json::Number(n), FieldType::Float) if n.as_f64().is_some() => from_json(j, f),
        _ => from_json(j, f),
    }
}

impl Store {
    pub fn new(schema: Schema) -> Store {
        let tables = schema
            .tables
            .iter()
            .map(|t| TableData::new(&t.name))
            .collect();
        Store {
            schema,
            tables,
            orphan_tables: BTreeMap::new(),
        }
    }

    pub fn table_data(&self, name: &str) -> Option<&TableData> {
        self.tables.iter().find(|t| t.name == name)
    }

    /// Serialize to (schema file, data file).
    pub fn save(&self) -> (String, String) {
        let mut tables = serde_json::Map::new();
        for (t, td) in self.schema.tables.iter().zip(&self.tables) {
            let mut rows = td.rows.clone();
            rows.sort_by_key(|r| r.id);
            let rows: Vec<Json> = rows
                .iter()
                .map(|r| {
                    let mut m = serde_json::Map::new();
                    if t.id_index().is_none() {
                        m.insert("_id".into(), json!(r.id));
                    }
                    for (f, v) in t.fields.iter().zip(&r.values) {
                        if *v != Value::Null {
                            m.insert(f.name.clone(), cell_to_json(v, f));
                        }
                    }
                    if !r.orphans.is_empty() {
                        m.insert("_orphans".into(), json!(r.orphans));
                    }
                    Json::Object(m)
                })
                .collect();
            tables.insert(t.name.clone(), json!({"next_id": td.next_id, "rows": rows}));
        }
        for (k, v) in &self.orphan_tables {
            tables.insert(k.clone(), v.clone());
        }
        let data = json!({"schema_version": self.schema.schema_version, "tables": tables});
        (
            self.schema.to_json(),
            serde_json::to_string_pretty(&data).unwrap(),
        )
    }

    /// Load by field name. Returns the store and warnings (bad cells, orphans, old version).
    pub fn load(schema_json: &str, data_json: &str) -> Result<(Store, Vec<String>), DataError> {
        let mut schema = Schema::from_json(schema_json)?;
        let data: Json = serde_json::from_str(data_json)?;
        let mut warnings = vec![];
        if schema.upgrade_links() | schema.upgrade_image_info() {
            warnings.push(UPGRADE_WARNING.to_string());
        }
        if let Some(v) = data["schema_version"].as_u64() {
            if (v as u32) < schema.schema_version {
                warnings.push(format!(
                    "data has schema_version {v}, schema is {}; no migration defined",
                    schema.schema_version
                ));
            }
        }
        let mut store = Store::new(schema);
        let empty = serde_json::Map::new();
        let tables = data["tables"].as_object().unwrap_or(&empty);
        for (name, tj) in tables {
            let Some(ti) = store.schema.tables.iter().position(|t| &t.name == name) else {
                warnings.push(format!("table '{name}' not in schema, kept as orphan"));
                store.orphan_tables.insert(name.clone(), tj.clone());
                continue;
            };
            let table = &store.schema.tables[ti];
            let mut td = TableData::new(name);
            td.next_id = tj["next_id"].as_u64().unwrap_or(1).max(1);
            let rows = tj["rows"].as_array().cloned().unwrap_or_default();
            for (n, rj) in rows.iter().enumerate() {
                let obj = rj.as_object().unwrap_or(&empty);
                let mut orphans: BTreeMap<String, Json> = match obj.get("_orphans") {
                    Some(Json::Object(m)) => m.clone().into_iter().collect(),
                    _ => BTreeMap::new(),
                };
                let mut values = vec![];
                for f in &table.fields {
                    match obj.get(&f.name) {
                        None => values.push(default_value(f)),
                        Some(j) => match cell_from_json(j, f) {
                            Ok(v) => values.push(v),
                            Err(e) => {
                                warnings.push(format!("{name} row {}: {e}", n + 1));
                                orphans.insert(f.name.clone(), j.clone());
                                values.push(Value::Null);
                            }
                        },
                    }
                }
                for (k, j) in obj {
                    if k != "_orphans" && k != "_id" && table.field_index(k).is_none() {
                        orphans.insert(k.clone(), j.clone());
                    }
                }
                let id = match obj
                    .get(if table.id_index().is_some() {
                        "id"
                    } else {
                        "_id"
                    })
                    .and_then(|j| j.as_u64())
                {
                    Some(i) => i,
                    None => td.next_id,
                };
                td.next_id = td.next_id.max(id + 1);
                if let Some(i) = table.id_index() {
                    values[i] = Value::Int(id as i64);
                }
                let mut row = Row::new(id, values);
                row.orphans = orphans;
                td.rows.push(row);
            }
            store.tables[ti] = td;
        }
        let n: usize = store
            .tables
            .iter()
            .flat_map(|t| &t.rows)
            .filter(|r| !r.orphans.is_empty())
            .count();
        if n > 0 {
            warnings.push(format!("{n} rows carry orphaned values"));
        }
        Ok((store, warnings))
    }

    /// Drop all orphaned values and tables. Explicit user action only.
    pub fn purge_orphans(&mut self) {
        self.orphan_tables.clear();
        for t in &mut self.tables {
            for r in &mut t.rows {
                r.orphans.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Schema {
        Schema::from_json(include_str!("../../../schemas/latent.json")).unwrap()
    }

    #[test]
    fn image_ref_links() {
        assert_eq!(
            film_row_for("images/roll_film/0012/x.jpg"),
            Some(("roll_film".into(), 12))
        );
        assert_eq!(film_row_for("foo/bar"), None);
        let sj = r#"{"name":"x","tables":[
            {"name":"f","role":"film","fields":[{"name":"images","type":"images"}]},
            {"name":"e","role":"enlargement","fields":[{"name":"src","type":"image_ref"}]},
            {"name":"i","role":"image_info","fields":[{"name":"src","type":"image_ref"},{"name":"tags","type":"text"}]}]}"#;
        let dj = r#"{"tables":{"e":{"next_id":3,"rows":[{"src":"images/f/0001/a.jpg"},{"src":"images/f/0001/b.jpg"},{"src":"images/f/0001/a.jpg"}]}}}"#;
        let (s, w) = Store::load(sj, dj).unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(enlargements_for(&s, "images/f/0001/a.jpg").len(), 2);
        assert!(enlargements_for(&s, "nope").is_empty());
        let (_, saved) = s.save();
        let (s2, _) = Store::load(sj, &saved).unwrap();
        assert_eq!(enlargements_for(&s2, "images/f/0001/b.jpg").len(), 1);
        assert_eq!(image_info_for(&s2, "images/f/0001/a.jpg"), None);
    }

    #[test]
    fn images_roundtrip() {
        let sj = r#"{"name":"x","tables":[{"name":"t","fields":[{"name":"p","type":"images"}]}]}"#;
        let dj = r#"{"tables":{"t":{"next_id":2,"rows":[{"p":["a/1.jpg","b.png"]}]}}}"#;
        let (s, w) = Store::load(sj, dj).unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(
            image_paths(&s.tables[0].rows[0].values[0]),
            vec!["a/1.jpg", "b.png"]
        );
        let (_, d2) = s.save();
        assert!(d2.contains("\"b.png\""));
        assert_eq!(Store::load(sj, &d2).unwrap().0, s);
    }

    #[test]
    fn image_paths_sorted_by_trailing_number() {
        let sorted = |s: &str| image_paths(&Value::Text(s.into()));
        assert_eq!(
            sorted("d/img_10.jpg\nd/img_2.jpg\nd/img_1.tif"),
            ["d/img_1.tif", "d/img_2.jpg", "d/img_10.jpg"]
        );
        // The number counts, not the prefix; names without a number come last.
        assert_eq!(
            sorted("z.jpg\nb_1.jpg\nA.jpg\na_2.jpg"),
            ["b_1.jpg", "a_2.jpg", "A.jpg", "z.jpg"]
        );
        // Equal numbers: by file name. Directories are ignored.
        assert_eq!(sorted("y/b-3.jpg\nx/a-3.jpg"), ["x/a-3.jpg", "y/b-3.jpg"]);
    }

    #[test]
    fn sample_schema_valid() {
        assert_eq!(schema().tables.len(), 4);
    }

    #[test]
    fn roll_film_csv() {
        let s = schema();
        let t = s.table("roll_film").unwrap();
        let td =
            TableData::from_csv(t, include_str!("../../../tests/fixtures/roll_film.csv")).unwrap();
        assert_eq!(td.rows.len(), 3);
        assert_eq!(td.next_id, 4);
        assert_eq!(td.rows[0].values[7], Value::Time(11 * 60 + 30));
        assert_eq!(td.rows[0].values[9], Value::Text("Strasse, Nacht".into()));
        assert_eq!(td.rows[1].values[7], Value::Null);
        assert_eq!(td.rows[1].values[9], Value::Text("Zeile 1\nZeile 2".into()));
        assert_eq!(td.rows[2].values[8], Value::Float(20.5));
        // export then re-import is stable
        let again = TableData::from_csv(t, &td.to_csv(t).unwrap()).unwrap();
        assert_eq!(again, td);
    }

    #[test]
    fn enlargement_csv_without_id() {
        let s = schema();
        let t = s.table("enlargement").unwrap();
        let td = TableData::from_csv(t, include_str!("../../../tests/fixtures/enlargement.csv"))
            .unwrap();
        assert_eq!(td.rows.len(), 1);
        assert_eq!(td.rows[0].id, 1);
        assert_eq!(td.rows[0].values[4], Value::Time(12));
        assert_eq!(
            td.rows[0].values[0],
            Value::Date(NaiveDate::from_ymd_opt(2020, 2, 1).unwrap())
        );
    }

    #[test]
    fn insert_and_roundtrip() {
        let s = schema();
        let mut store = Store::new(s.clone());
        let t = s.table("roll_film").unwrap();
        let mut v = TableData::blank_values(t);
        v[2] = Value::Text("HP5".into());
        v[7] = Value::Time(90);
        let td = &mut store.tables[0];
        assert_eq!(td.insert(t, v.clone()), 1);
        assert_eq!(td.insert(t, v), 2);
        td.remove(1);
        let (sj, dj) = store.save();
        assert!(dj.contains("\"01:30\""));
        let (back, w) = Store::load(&sj, &dj).unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(back.tables[0].next_id, 3);
        assert_eq!(back, store);
    }

    fn small(fields: &str) -> String {
        format!(
            r#"{{"name":"x","schema_version":2,"tables":[{{"name":"t","fields":[{fields}]}}]}}"#
        )
    }
    const A: &str = r#"{"name":"a","type":"text"}"#;
    const B: &str = r#"{"name":"b","type":"int"}"#;
    const DATA: &str =
        r#"{"schema_version":2,"tables":{"t":{"next_id":2,"rows":[{"a":"x","b":5}]}}}"#;

    #[test]
    fn reorder_fields_keeps_data() {
        let (s, _) = Store::load(&small(&format!("{B},{A}")), DATA).unwrap();
        assert_eq!(
            s.tables[0].rows[0].values,
            vec![Value::Int(5), Value::Text("x".into())]
        );
    }

    #[test]
    fn added_field_gets_default() {
        let c = r#"{"name":"c","type":"int","default":7}"#;
        let (s, _) = Store::load(&small(&format!("{A},{B},{c}")), DATA).unwrap();
        assert_eq!(s.tables[0].rows[0].values[2], Value::Int(7));
    }

    #[test]
    fn removed_field_survives_as_orphan_until_purged() {
        let (s, w) = Store::load(&small(A), DATA).unwrap();
        assert!(w.iter().any(|m| m.contains("orphan")));
        let (sj, dj) = s.save();
        let (mut s2, _) = Store::load(&sj, &dj).unwrap();
        assert_eq!(s2.tables[0].rows[0].orphans["b"], json!(5));
        s2.purge_orphans();
        assert!(!s2.save().1.contains("_orphans"));
    }

    #[test]
    fn bad_cell_is_reported_not_fatal() {
        let d = r#"{"tables":{"t":{"rows":[{"a":"x","b":"oops"}]}}}"#;
        let (s, w) = Store::load(&small(&format!("{A},{B}")), d).unwrap();
        assert!(w[0].contains("t row 1") && w[0].contains("b"), "{w:?}");
        assert_eq!(s.tables[0].rows[0].orphans["b"], json!("oops"));
    }

    #[test]
    fn bad_csv_cell_reports_row() {
        let s = schema();
        let t = s.table("roll_film").unwrap();
        let e = TableData::from_csv(t, "1,notadate,x\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("row 1") && e.contains("date"), "{e}");
    }
}

/// Native file IO: atomic write (temp + rename), keeping one `.bak` of the previous file.
#[cfg(not(target_arch = "wasm32"))]
pub mod fs {
    use super::*;
    use std::path::Path;

    fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        if path.exists() {
            let mut bak = path.as_os_str().to_owned();
            bak.push(".bak");
            std::fs::copy(path, bak)?;
        }
        std::fs::rename(tmp, path)
    }

    fn same_file(a: &Path, b: &Path) -> bool {
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(x), Ok(y)) if x.len() == y.len() => {
                matches!((std::fs::read(a), std::fs::read(b)), (Ok(p), Ok(q)) if p == q)
            }
            _ => false,
        }
    }

    /// Copy `source` into `<archive>/images/<table>/<row id>/` and return the archive-relative
    /// path (forward slashes). Identical existing files are reused; name clashes get a suffix.
    pub fn import_image(
        archive: &Path,
        table: &str,
        row_id: u64,
        source: &Path,
    ) -> std::io::Result<String> {
        let name = source
            .file_name()
            .ok_or_else(|| std::io::Error::other("no file name"))?;
        let rel_dir = format!("images/{table}/{row_id:04}");
        let dir = archive.join(&rel_dir);
        std::fs::create_dir_all(&dir)?;
        let (stem, ext) = (
            source
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            source
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default(),
        );
        let mut n = 0;
        loop {
            let fname = if n == 0 {
                name.to_string_lossy().into_owned()
            } else {
                format!("{stem}-{n}{ext}")
            };
            let target = dir.join(&fname);
            if !target.exists() {
                std::fs::copy(source, &target)?;
                return Ok(format!("{rel_dir}/{fname}"));
            }
            if same_file(source, &target) {
                return Ok(format!("{rel_dir}/{fname}"));
            }
            n += 1;
        }
    }

    /// Files under `<archive>/images` that no row references.
    pub fn unused_images(store: &Store, archive: &Path) -> Vec<std::path::PathBuf> {
        let mut used = std::collections::HashSet::new();
        for (t, td) in store.schema.tables.iter().zip(&store.tables) {
            for (i, f) in t.fields.iter().enumerate() {
                if matches!(f.ty, FieldType::Images | FieldType::ImageRef) {
                    for r in &td.rows {
                        used.extend(
                            image_paths(&r.values[i])
                                .into_iter()
                                .map(|p| archive.join(p)),
                        );
                    }
                }
            }
        }
        let mut out = vec![];
        let mut stack = vec![archive.join("images")];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if !used.contains(&p) {
                    out.push(p);
                }
            }
        }
        out.sort();
        out
    }

    pub fn save(store: &Store, schema_path: &Path, data_path: &Path) -> std::io::Result<()> {
        let (s, d) = store.save();
        write_atomic(schema_path, &s)?;
        write_atomic(data_path, &d)
    }

    pub fn load(schema_path: &Path, data_path: &Path) -> Result<(Store, Vec<String>), DataError> {
        let io = |e: std::io::Error| DataError::Parse(e.to_string());
        let s = std::fs::read_to_string(schema_path).map_err(io)?;
        let d = std::fs::read_to_string(data_path).map_err(io)?;
        Store::load(&s, &d)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod fs_tests {
    #[test]
    fn save_load_with_backup() {
        let dir = std::env::temp_dir().join("latent_fs_test");
        std::fs::create_dir_all(&dir).unwrap();
        let (s, d) = (dir.join("a.schema.json"), dir.join("a.data.json"));
        let schema =
            latent_schema::Schema::from_json(include_str!("../../../schemas/latent.json")).unwrap();
        let store = crate::Store::new(schema);
        super::fs::save(&store, &s, &d).unwrap();
        super::fs::save(&store, &s, &d).unwrap();
        assert!(dir.join("a.data.json.bak").exists());
        assert_eq!(super::fs::load(&s, &d).unwrap().0, store);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod image_tests {
    use super::*;

    #[test]
    fn import_and_unused() {
        let root = std::env::temp_dir().join("latent_img_test");
        let _ = std::fs::remove_dir_all(&root);
        let (src, arch) = (root.join("src"), root.join("arch"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.jpg"), b"one").unwrap();
        let r1 = fs::import_image(&arch, "roll_film", 1, &src.join("a.jpg")).unwrap();
        assert_eq!(r1, "images/roll_film/0001/a.jpg");
        // identical file is reused
        assert_eq!(
            fs::import_image(&arch, "roll_film", 1, &src.join("a.jpg")).unwrap(),
            r1
        );
        // different content with the same name gets a suffix
        std::fs::write(src.join("a.jpg"), b"two").unwrap();
        let r2 = fs::import_image(&arch, "roll_film", 1, &src.join("a.jpg")).unwrap();
        assert_eq!(r2, "images/roll_film/0001/a-1.jpg");
        assert!(fs::import_image(&arch, "roll_film", 1, &src.join("missing.jpg")).is_err());

        let sj = r#"{"name":"x","tables":[{"name":"roll_film","fields":[{"name":"p","type":"images"}]}]}"#;
        let mut store = Store::new(Schema::from_json(sj).unwrap());
        let t = store.schema.tables[0].clone();
        store.tables[0].insert(&t, vec![Value::Text(r1)]);
        let unused = fs::unused_images(&store, &arch);
        assert_eq!(unused, vec![arch.join(&r2)]);
    }
}
