//! UI-free engine: CRUD with undo/redo, sorting and filtering over a `Store`.
use fa_data::{format_value, Row, Store, Value};

#[derive(Debug, Clone)]
enum Edit {
    Insert { table: usize, index: usize, row: Row },
    Delete { table: usize, index: usize, row: Row },
    Update { table: usize, id: u64, old: Vec<Value>, new: Vec<Value> },
}

#[derive(Debug, Clone, Default)]
pub struct Query {
    /// (field index, ascending)
    pub sort: Option<(usize, bool)>,
    /// Case-insensitive substring matched against any displayed cell.
    pub search: String,
    /// Per-column substring filters (field index, text).
    pub column_filters: Vec<(usize, String)>,
    /// Exact match on the displayed value (enum columns).
    pub equals: Vec<(usize, String)>,
    /// Inclusive range as typed text (from, to), parsed per field type (numbers, dates, times).
    pub ranges: Vec<(usize, String, String)>,
}

pub struct Engine {
    pub store: Store,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    pub dirty: bool,
}

impl Engine {
    pub fn new(store: Store) -> Self {
        Engine { store, undo: vec![], redo: vec![], dirty: false }
    }

    fn table_index(&self, name: &str) -> Option<usize> {
        self.store.schema.tables.iter().position(|t| t.name == name)
    }

    pub fn insert(&mut self, table: &str, values: Vec<Value>) -> Option<u64> {
        let ti = self.table_index(table)?;
        let t = &self.store.schema.tables[ti];
        let td = &mut self.store.tables[ti];
        let id = td.insert(t, values);
        let index = td.rows.len() - 1;
        let row = td.rows[index].clone();
        self.push(Edit::Insert { table: ti, index, row });
        Some(id)
    }

    pub fn update(&mut self, table: &str, id: u64, values: Vec<Value>) -> bool {
        let Some(ti) = self.table_index(table) else { return false };
        let t = &self.store.schema.tables[ti];
        let Some(row) = self.store.tables[ti].get_mut(id) else { return false };
        let mut new = values;
        if let Some(i) = t.id_index() {
            new[i] = Value::Int(id as i64); // id is immutable
        }
        if row.values == new {
            return false;
        }
        let old = std::mem::replace(&mut row.values, new.clone());
        self.push(Edit::Update { table: ti, id, old, new });
        true
    }

    pub fn delete(&mut self, table: &str, id: u64) -> bool {
        let Some(ti) = self.table_index(table) else { return false };
        let td = &mut self.store.tables[ti];
        let Some(index) = td.rows.iter().position(|r| r.id == id) else { return false };
        let row = td.rows.remove(index);
        self.push(Edit::Delete { table: ti, index, row });
        true
    }

    fn push(&mut self, e: Edit) {
        self.undo.push(e);
        self.redo.clear();
        self.dirty = true;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn apply(&mut self, e: &Edit, forward: bool) {
        match (e, forward) {
            (Edit::Insert { table, index, row }, false) | (Edit::Delete { table, index, row }, true) => {
                let rows = &mut self.store.tables[*table].rows;
                let p = rows.iter().position(|r| r.id == row.id).unwrap_or(*index);
                rows.remove(p.min(rows.len() - 1));
            }
            (Edit::Insert { table, index, row }, true) | (Edit::Delete { table, index, row }, false) => {
                let rows = &mut self.store.tables[*table].rows;
                rows.insert((*index).min(rows.len()), row.clone());
            }
            (Edit::Update { table, id, old, new }, fwd) => {
                if let Some(r) = self.store.tables[*table].get_mut(*id) {
                    r.values = if fwd { new.clone() } else { old.clone() };
                }
            }
        }
        self.dirty = true;
    }

    pub fn undo(&mut self) -> bool {
        let Some(e) = self.undo.pop() else { return false };
        self.apply(&e, false);
        self.redo.push(e);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(e) = self.redo.pop() else { return false };
        self.apply(&e, true);
        self.undo.push(e);
        true
    }

    /// Row ids of `table` matching the query, in display order.
    pub fn view(&self, table: &str, q: &Query) -> Vec<u64> {
        let Some(ti) = self.table_index(table) else { return vec![] };
        let t = &self.store.schema.tables[ti];
        let cell = |r: &Row, i: usize| format_value(&r.values[i], &t.fields[i]).to_lowercase();
        let search = q.search.to_lowercase();
        let mut rows: Vec<&Row> = self.store.tables[ti]
            .rows
            .iter()
            .filter(|r| {
                (search.is_empty() || (0..t.fields.len()).any(|i| cell(r, i).contains(&search)))
                    && q.column_filters
                        .iter()
                        .all(|(i, s)| *i >= t.fields.len() || cell(r, *i).contains(&s.to_lowercase()))
                    && q.equals.iter().all(|(i, s)| *i >= t.fields.len() || s.is_empty() || cell(r, *i) == s.to_lowercase())
                    && q.ranges.iter().all(|(i, lo, hi)| *i >= t.fields.len() || in_range(&r.values[*i], &t.fields[*i], lo, hi))
            })
            .collect();
        if let Some((i, asc)) = q.sort.filter(|(i, _)| *i < t.fields.len()) {
            rows.sort_by(|a, b| {
                let o = a.values[i].cmp_value(&b.values[i]);
                (if asc { o } else { o.reverse() }).then(a.id.cmp(&b.id))
            });
        }
        rows.iter().map(|r| r.id).collect()
    }
}

fn in_range(v: &Value, f: &fa_schema::Field, lo: &str, hi: &str) -> bool {
    let (lo, hi) = (fa_data::parse_value(lo, f).unwrap_or(Value::Null), fa_data::parse_value(hi, f).unwrap_or(Value::Null));
    if lo == Value::Null && hi == Value::Null {
        return true;
    }
    if *v == Value::Null {
        return false;
    }
    (lo == Value::Null || v.cmp_value(&lo).is_ge()) && (hi == Value::Null || v.cmp_value(&hi).is_le())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fa_data::TableData;
    use fa_schema::Schema;

    fn engine() -> Engine {
        let s = Schema::from_json(include_str!("../../../schemas/filmarchiv.json")).unwrap();
        Engine::new(Store::new(s))
    }
    fn vals(e: &Engine, name: &str, iso: i64) -> Vec<Value> {
        let t = e.store.schema.table("roll_film").unwrap();
        let mut v = TableData::blank_values(t);
        v[2] = Value::Text(name.into());
        v[5] = Value::Int(iso);
        v
    }

    #[test]
    fn crud_undo_redo() {
        let mut e = engine();
        let a = e.insert("roll_film", vals(&e, "HP5", 400)).unwrap();
        let b = e.insert("roll_film", vals(&e, "Acros", 100)).unwrap();
        assert_eq!((a, b), (1, 2));
        let mut v = vals(&e, "HP5 pushed", 1600);
        v[0] = Value::Int(99); // ignored
        assert!(e.update("roll_film", a, v));
        assert_eq!(e.store.tables[0].rows[0].values[0], Value::Int(1));
        assert!(e.delete("roll_film", a));
        assert_eq!(e.store.tables[0].rows.len(), 1);
        assert!(e.undo()); // delete
        assert_eq!(e.store.tables[0].rows[0].id, 1);
        assert!(e.undo()); // update
        assert_eq!(e.store.tables[0].rows[0].values[5], Value::Int(400));
        assert!(e.undo() && e.undo()); // both inserts
        assert!(e.store.tables[0].rows.is_empty() && !e.undo());
        assert!(e.redo() && e.redo() && e.redo() && e.redo());
        assert_eq!(e.store.tables[0].rows.len(), 1);
        assert!(!e.can_redo());
        // new edit clears redo
        e.undo();
        e.insert("roll_film", vals(&e, "X", 50));
        assert!(!e.can_redo());
    }

    #[test]
    fn typed_filters() {
        let mut e = engine();
        for (n, i) in [("HP5", 400), ("acros", 100), ("Tri-X", 800)] {
            e.insert("roll_film", vals(&e, n, i));
        }
        let q = Query { ranges: vec![(5, "200".into(), "".into())], ..Default::default() };
        assert_eq!(e.view("roll_film", &q), vec![1, 3]);
        let q = Query { ranges: vec![(5, "".into(), "400".into())], ..Default::default() };
        assert_eq!(e.view("roll_film", &q), vec![1, 2]);
        let q = Query { equals: vec![(2, "acros".into())], ..Default::default() };
        assert_eq!(e.view("roll_film", &q), vec![2]);
        // rows default to today's date
        let q = Query { ranges: vec![(1, "01.01.2000".into(), "".into())], ..Default::default() };
        assert_eq!(e.view("roll_film", &q).len(), 3);
        let q = Query { ranges: vec![(1, "01.01.2999".into(), "".into())], ..Default::default() };
        assert!(e.view("roll_film", &q).is_empty());
    }

    #[test]
    fn id_range() {
        let mut e = engine();
        for n in ["a", "b", "c", "d"] {
            e.insert("roll_film", vals(&e, n, 400));
        }
        let q = Query { ranges: vec![(0, "2".into(), "3".into())], ..Default::default() };
        assert_eq!(e.view("roll_film", &q), vec![2, 3]);
        let q = Query { ranges: vec![(0, "".into(), "2".into())], ..Default::default() };
        assert_eq!(e.view("roll_film", &q), vec![1, 2]);
        let q = Query { ranges: vec![(0, "3".into(), "".into())], ..Default::default() };
        assert_eq!(e.view("roll_film", &q), vec![3, 4]);
    }

    #[test]
    fn sort_and_filter() {
        let mut e = engine();
        for (n, i) in [("HP5", 400), ("acros", 100), ("Tri-X", 400)] {
            e.insert("roll_film", vals(&e, n, i));
        }
        let q = Query { sort: Some((2, true)), ..Default::default() };
        assert_eq!(e.view("roll_film", &q), vec![2, 1, 3]); // case-insensitive
        let q = Query { sort: Some((5, false)), ..Default::default() };
        assert_eq!(e.view("roll_film", &q), vec![1, 3, 2]); // stable on ties
        let q = Query { search: "TRI".into(), ..Default::default() };
        assert_eq!(e.view("roll_film", &q), vec![3]);
        let q = Query { column_filters: vec![(5, "400".into())], ..Default::default() };
        assert_eq!(e.view("roll_film", &q), vec![1, 3]);
    }
}
