//! Typed values, rows and tables driven by an `fa-schema` schema, plus CSV/JSON IO.
use chrono::NaiveDate;
use fa_schema::{Field, FieldType, Schema, Table};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use std::cmp::Ordering;

#[derive(Debug, thiserror::Error)]
pub enum DataError {
    #[error("{0}")]
    Parse(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("csv: {0}")]
    Csv(#[from] csv::Error),
    #[error("schema: {0}")]
    Schema(#[from] fa_schema::SchemaError),
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
    Some(if fmt == "%H:%M" { a * 3600 + b * 60 } else { a * 60 + b })
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
        FieldType::Bool => Value::Bool(matches!(s.to_lowercase().as_str(), "1" | "true" | "yes" | "ja")),
        FieldType::Date => Value::Date(
            NaiveDate::parse_from_str(s, f.date_format()).map_err(|_| err("date"))?,
        ),
        FieldType::Time => Value::Time(parse_time(s, f.time_format()).ok_or_else(|| err("time"))?),
        FieldType::Text | FieldType::Enum => Value::Text(s.to_string()),
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
        (Json::String(s), FieldType::Text | FieldType::Enum) => Value::Text(s.clone()),
        (Json::Number(n), FieldType::Int | FieldType::Id) => Value::Int(n.as_i64().ok_or_else(bad)?),
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableData {
    pub name: String,
    pub next_id: u64,
    pub rows: Vec<Row>,
}

impl TableData {
    pub fn new(name: &str) -> Self {
        TableData { name: name.into(), next_id: 1, rows: vec![] }
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
        self.rows.push(Row { id, values });
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
            let rec: Vec<String> =
                table.fields.iter().zip(&r.values).map(|(f, v)| format_value(v, f)).collect();
            w.write_record(&rec)?;
        }
        let bytes = w.into_inner().map_err(|e| DataError::Parse(e.to_string()))?;
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
                    td.rows.push(Row { id, values });
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
}

#[derive(Serialize, Deserialize)]
struct FileFormat {
    schema: Schema,
    data: Vec<FileTable>,
}
#[derive(Serialize, Deserialize)]
struct FileTable {
    table: String,
    next_id: u64,
    rows: Vec<Vec<Json>>,
}

impl Store {
    pub fn new(schema: Schema) -> Store {
        let tables = schema.tables.iter().map(|t| TableData::new(&t.name)).collect();
        Store { schema, tables }
    }

    pub fn table_data(&self, name: &str) -> Option<&TableData> {
        self.tables.iter().find(|t| t.name == name)
    }

    pub fn to_json(&self) -> String {
        let data = self
            .tables
            .iter()
            .map(|t| FileTable {
                table: t.name.clone(),
                next_id: t.next_id,
                rows: t.rows.iter().map(|r| r.values.iter().map(to_json).collect()).collect(),
            })
            .collect();
        serde_json::to_string_pretty(&FileFormat { schema: self.schema.clone(), data }).unwrap()
    }

    /// Load a single-file archive (schema + data).
    pub fn from_json(s: &str) -> Result<Store, DataError> {
        let ff: FileFormat = serde_json::from_str(s)?;
        ff.schema.validate()?;
        Self::from_parts(ff.schema, ff.data)
    }

    /// Load data against a (possibly newer) schema, matching columns by position
    /// in the stored rows; missing columns become Null, extra ones are dropped.
    fn from_parts(schema: Schema, data: Vec<FileTable>) -> Result<Store, DataError> {
        let mut store = Store::new(schema);
        for ft in data {
            let ti = store
                .schema
                .tables
                .iter()
                .position(|t| t.name == ft.table)
                .ok_or_else(|| DataError::UnknownTable(ft.table.clone()))?;
            let table = &store.schema.tables[ti];
            let mut td = TableData::new(&ft.table);
            td.next_id = ft.next_id.max(1);
            for (n, row) in ft.rows.iter().enumerate() {
                let values = table
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| from_json(row.get(i).unwrap_or(&Json::Null), f))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| DataError::Parse(format!("{} row {}: {e}", ft.table, n + 1)))?;
                let id = match table.id_index().map(|i| &values[i]) {
                    Some(Value::Int(i)) => *i as u64,
                    _ => td.next_id + n as u64,
                };
                td.next_id = td.next_id.max(id + 1);
                td.rows.push(Row { id, values });
            }
            store.tables[ti] = td;
        }
        Ok(store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Schema {
        Schema::from_json(include_str!("../../../schemas/filmarchiv.json")).unwrap()
    }

    #[test]
    fn sample_schema_valid() {
        assert_eq!(schema().tables.len(), 3);
    }

    #[test]
    fn roll_film_csv() {
        let s = schema();
        let t = s.table("roll_film").unwrap();
        let td = TableData::from_csv(t, include_str!("../../../tests/fixtures/roll_film.csv")).unwrap();
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
        let td = TableData::from_csv(t, include_str!("../../../tests/fixtures/enlargement.csv")).unwrap();
        assert_eq!(td.rows.len(), 1);
        assert_eq!(td.rows[0].id, 1);
        assert_eq!(td.rows[0].values[4], Value::Time(12));
        assert_eq!(
            td.rows[0].values[0],
            Value::Date(NaiveDate::from_ymd_opt(2020, 2, 1).unwrap())
        );
    }

    #[test]
    fn insert_assigns_ids_and_json_roundtrip() {
        let s = schema();
        let mut store = Store::new(s.clone());
        let t = s.table("roll_film").unwrap();
        let mut v = TableData::blank_values(t);
        v[2] = Value::Text("HP5".into());
        let td = &mut store.tables[0];
        assert_eq!(td.insert(t, v.clone()), 1);
        assert_eq!(td.insert(t, v), 2);
        assert_eq!(td.rows[1].values[0], Value::Int(2));
        td.remove(1);
        let back = Store::from_json(&store.to_json()).unwrap();
        assert_eq!(back.tables[0].next_id, 3);
        assert_eq!(back, store);
    }

    #[test]
    fn bad_cell_reports_row() {
        let s = schema();
        let t = s.table("roll_film").unwrap();
        let e = TableData::from_csv(t, "1,notadate,x\n").unwrap_err().to_string();
        assert!(e.contains("row 1") && e.contains("date"), "{e}");
    }
}
