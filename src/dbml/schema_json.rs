//! The schema-parity contract (parity/PARITY_SCHEMA_SPEC.md): our
//! document rendered as the same normalized JSON their
//! `Compiler.interpretFile` produces (token positions stripped in v1).
//! The runner diffs the two per corpus case — this is the parser's
//! output gate, and the formatter's model-preservation oracle.

use serde_json::{json, Map, Value};

use super::ast::{self, Document, RecordValue};


/// their TokenPosition with offsets only: `{"start": {"offset": N}, "end": {"offset": M}}`
fn token_json(span: std::ops::Range<usize>) -> Value {
    json!({"start": {"offset": span.start}, "end": {"offset": span.end}})
}

/// The declaration content end: our spans can carry trailing same-line
/// whitespace their tokens exclude.
fn trimmed_end(src: &str, end: usize) -> usize {
    src[..end.min(src.len())].trim_end().len()
}

pub fn to_database_json(doc: &Document, src: &str) -> Value {
    let mut db = Map::new();
    db.insert("token".into(), token_json(0..src.len()));
    db.insert("schemas".into(), json!([]));
    db.insert(
        "tables".into(),
        Value::Array(doc.tables.iter().map(|t| table_json(t, src)).collect()),
    );
    db.insert(
        "notes".into(),
        Value::Array(doc.notes.iter().map(note_json).collect()),
    );
    db.insert(
        "refs".into(),
        Value::Array(refs_json(doc, src).into_iter().collect()),
    );
    db.insert("deps".into(), json!([]));
    db.insert(
        "enums".into(),
        Value::Array(doc.enums.iter().map(enum_json).collect()),
    );
    db.insert(
        "tableGroups".into(),
        Value::Array(doc.table_groups.iter().map(group_json).collect()),
    );
    // their aliases: one entry per table alias
    let aliases: Vec<Value> = doc
        .tables
        .iter()
        .filter_map(|t| {
            t.alias.as_ref().map(|a| {
                json!({
                    "name": a,
                    "kind": "table",
                    "value": {"tableName": t.name, "schemaName": t.schema},
                })
            })
        })
        .collect();
    db.insert("aliases".into(), json!(aliases));
    db.insert("tablePartials".into(), json!([]));
    let records: Vec<Value> = doc
        .tables
        .iter()
        .filter_map(|t| t.records.as_ref().map(|r| record_json(t, r, doc, src)))
        .collect();
    db.insert("records".into(), json!(records));
    db.insert("diagramViews".into(), json!([]));
    db.insert(
        "externals".into(),
        json!({"tables": [], "enums": [], "tableGroups": [], "tablePartials": [], "notes": []}),
    );
    if let Some(p) = &doc.project {
        let mut proj = Map::new();
        proj.insert("enums".into(), json!([]));
        proj.insert("refs".into(), json!([]));
        proj.insert("tableGroups".into(), json!([]));
        proj.insert("tables".into(), json!([]));
        proj.insert("tablePartials".into(), json!([]));
        proj.insert("name".into(), json!(p.name));
        if let Some(dt) = &p.database_type {
            proj.insert("database_type".into(), json!(dt));
        }
        if let Some(n) = &p.note {
            proj.insert("note".into(), json!({"value": n}));
        }
        db.insert("project".into(), Value::Object(proj));
    }
    Value::Object(db)
}

fn table_json(t: &ast::Table, src: &str) -> Value {
    let mut m = Map::new();
    m.insert("token".into(), token_json(t.span.start..trimmed_end(src, t.span.end)));
    m.insert("name".into(), json!(t.name));
    m.insert("schemaName".into(), json!(t.schema));
    m.insert("alias".into(), json!(t.alias));
    // their table interpretation: 2+ column-level [pk] columns merge into
    // a synthetic composite pk index and the column flags RESET; a single
    // column-level [pk] stays a field flag
    let pk_cols: Vec<&ast::Column> = t.columns.iter().filter(|c| c.settings.pk).collect();
    let merged = pk_cols.len() > 1;
    let fields: Vec<Value> = t
        .columns
        .iter()
        .map(|c| column_json(c, merged && c.settings.pk, src))
        .collect();
    m.insert("fields".into(), json!(fields));
    m.insert(
        "checks".into(),
        Value::Array(
            t.check_entries
                .iter()
                .map(|c| {
                    let mut e = Map::new();
                    e.insert("expression".into(), json!(c.expression));
                    if let Some(n) = &c.name {
                        e.insert("name".into(), json!(n));
                    }
                    Value::Object(e)
                })
                .collect(),
        ),
    );
    m.insert("partials".into(), json!([]));
    let mut indexes: Vec<Value> = t.indexes.iter().map(|ix| index_json(ix, src)).collect();
    if merged {
        // their synthetic composite pk index: the token is their {-1,-1}
        // sentinel (no real declaration) and the column entries carry the
        // source columns' spans
        indexes.push(json!({
            "token": {"start": {"offset": -1}, "end": {"offset": -1}},
            "columns": pk_cols
                .iter()
                .map(|c| {
                    json!({
                        "token": token_json(c.span.clone()),
                        "value": c.name,
                        "type": "column",
                    })
                })
                .collect::<Vec<_>>(),
            "pk": true,
        }));
    }
    m.insert("indexes".into(), json!(indexes));
    m.insert("metadata".into(), custom_json(&t.custom));
    if let Some(h) = &t.header_color {
        m.insert("headerColor".into(), json!(h));
    }
    if let Some(n) = &t.note {
        m.insert("note".into(), json!({"value": n}));
    }
    Value::Object(m)
}

fn column_json(c: &ast::Column, pk_reset: bool, src: &str) -> Value {
    let mut m = Map::new();
    m.insert("token".into(), token_json(c.span.start..trimmed_end(src, c.span.end)));
    m.insert("name".into(), json!(c.name));
    let (schema, type_name, args) = split_type(&c.ty);
    let mut ty = Map::new();
    ty.insert("schemaName".into(), json!(schema));
    ty.insert("type_name".into(), json!(type_name));
    ty.insert("args".into(), json!(args));
    m.insert("type".into(), Value::Object(ty));
    m.insert("pk".into(), json!(c.settings.pk && !pk_reset));
    m.insert("unique".into(), json!(c.settings.unique));
    if let Some(nn) = c.settings.not_null {
        m.insert("not_null".into(), json!(nn));
    }
    if c.settings.increment {
        m.insert("increment".into(), json!(true));
    }
    if let Some(d) = &c.settings.default {
        m.insert("dbdefault".into(), typed_value(d));
    }
    if let Some(n) = &c.settings.note {
        m.insert("note".into(), json!({"value": n}));
    }
    m.insert("metadata".into(), custom_json(&c.settings.custom));
    let inline_refs: Vec<Value> = c
        .settings
        .inline_ref
        .as_ref()
        .map(|ir| {
            let op = super::validate::written_op(ir.card, ir.left_optional, ir.right_optional);
            vec![json!({
                "token": token_json(ir.attr_start..ir.to.span.end),
                "schemaName": ir.to.schema,
                "tableName": ir.to.table,
                "fieldNames": ir.to.columns,
                "relation": op,
            })]
        })
        .unwrap_or_default();
    m.insert("inline_refs".into(), json!(inline_refs));
    m.insert("inline_deps".into(), json!([]));
    let checks: Vec<Value> = c
        .settings
        .check
        .as_ref()
        .map(|e| {
            // the stored form keeps the backticks for the formatter; the
            // contract carries the bare expression
            let bare = e.trim_start_matches('`').trim_end_matches('`');
            vec![json!({"expression": bare})]
        })
        .unwrap_or_default();
    m.insert("checks".into(), json!(checks));
    Value::Object(m)
}

fn index_json(ix: &ast::Index, src: &str) -> Value {
    let mut m = Map::new();
    m.insert("token".into(), token_json(ix.span.start..trimmed_end(src, ix.span.end)));
    let columns: Vec<Value> = ix
        .columns
        .iter()
        .zip(ix.expressions.iter())
        .zip(ix.column_spans.iter())
        .map(|((c, is_expr), span)| {
            json!({
                "token": token_json(span.clone()),
                "value": c,
                "type": if *is_expr { "expression" } else { "column" },
            })
        })
        .collect();
    m.insert("columns".into(), json!(columns));
    if ix.unique {
        m.insert("unique".into(), json!(true));
    }
    if ix.pk {
        m.insert("pk".into(), json!(true));
    }
    if let Some(n) = &ix.name {
        m.insert("name".into(), json!(n));
    }
    if let Some(t) = &ix.index_type {
        m.insert("type".into(), json!(t));
    }
    Value::Object(m)
}

fn refs_json(doc: &Document, src: &str) -> Vec<Value> {
    let mut out = Vec::new();
    // standalone refs keep their written endpoint order
    for r in &doc.refs {
        let (l, rt) = super::validate::op_cards(super::validate::written_op(
            r.card,
            r.left_optional,
            r.right_optional,
        ));
        let mut m = Map::new();
        m.insert("token".into(), token_json(r.span.clone()));
        m.insert("name".into(), json!(r.name));
        m.insert("schemaName".into(), json!(r.from.schema.clone()));
        m.insert(
            "endpoints".into(),
            json!([
                {"token": token_json(r.from.span.clone()), "schemaName": r.from.schema, "tableName": r.from.table,
                 "fieldNames": r.from.columns, "relation": card_str(l)},
                {"token": token_json(r.to.span.clone()), "schemaName": r.to.schema, "tableName": r.to.table,
                 "fieldNames": r.to.columns, "relation": card_str(rt)},
            ]),
        );
        if let Some(d) = &r.on_delete {
            m.insert("onDelete".into(), json!(d.to_ascii_lowercase()));
        }
        if let Some(u) = &r.on_update {
            m.insert("onUpdate".into(), json!(u.to_ascii_lowercase()));
        }
        if let Some(c) = &r.color {
            m.insert("color".into(), json!(c));
        }
        if r.on_delete.is_some() || r.on_update.is_some() || r.color.is_some() {
            m.insert("metadata".into(), json!({}));
        }
        out.push(Value::Object(m));
    }
    // inline refs store as the REVERSED ref: target first, the op's
    // cardinalities swapped (probe-verified on their interpretFile)
    for t in &doc.tables {
        for c in &t.columns {
            let Some(ir) = &c.settings.inline_ref else { continue };
            let (l, rt) = super::validate::op_cards(super::validate::written_op(
                ir.card,
                ir.left_optional,
                ir.right_optional,
            ));
            let mut ir_m = Map::new();
            ir_m.insert(
                "token".into(),
                token_json(ir.attr_start..ir.to.span.end),
            );
            ir_m.insert(
                "endpoints".into(),
                json!([
                    {"token": token_json(ir.attr_start..ir.to.span.end), "schemaName": ir.to.schema, "tableName": ir.to.table,
                     "fieldNames": ir.to.columns, "relation": card_str(rt)},
                    {"token": token_json(c.span.start..trimmed_end(src, c.span.end)), "schemaName": t.schema, "tableName": t.name,
                     "fieldNames": [c.name], "relation": card_str(l)},
                ]),
            );
            out.push(Value::Object(ir_m));
        }
    }
    out
}

fn enum_json(e: &ast::Enum) -> Value {
    let values: Vec<Value> = e
        .values
        .iter()
        .map(|v| {
            let mut m = Map::new();
            m.insert("token".into(), token_json(v.name_span.clone()));
            m.insert("name".into(), json!(v.name));
            if let Some(n) = &v.note {
                m.insert("note".into(), json!({"value": n}));
            }
            Value::Object(m)
        })
        .collect();
    let mut m = Map::new();
    m.insert("token".into(), token_json(e.span.clone()));
    m.insert("name".into(), json!(e.name));
    m.insert("schemaName".into(), json!(e.schema));
    m.insert("values".into(), json!(values));
    Value::Object(m)
}

fn group_json(g: &ast::TableGroup) -> Value {
    let tables: Vec<Value> = g
        .tables
        .iter()
        .map(|m| match m.split_once('.') {
            Some((s, n)) => json!({"name": n, "schemaName": s}),
            None => json!({"name": m, "schemaName": null}),
        })
        .collect();
    let mut m = Map::new();
    m.insert("token".into(), token_json(g.span.clone()));
    m.insert("tables".into(), json!(tables));
    m.insert("name".into(), json!(g.name));
    m.insert("schemaName".into(), json!(Value::Null));
    if let Some(c) = &g.color {
        m.insert("color".into(), json!(c));
    }
    if let Some(n) = &g.note {
        m.insert("note".into(), json!({"value": n}));
    }
    m.insert("metadata".into(), custom_json(&g.custom));
    Value::Object(m)
}

fn note_json(n: &ast::StickyNote) -> Value {
    json!({"token": token_json(n.span.clone()), "name": n.name, "content": n.text, "metadata": {}})
}

fn record_json(t: &ast::Table, r: &ast::TableRecords, doc: &Document, src: &str) -> Value {
    // resolve the header: explicit columns or the table's own order
    let columns: Vec<String> = if r.columns.is_empty() {
        doc.tables
            .iter()
            .find(|x| x.name == t.name)
            .map(|x| x.columns.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default()
    } else {
        r.columns.clone()
    };
    let col_type = |name: &str| -> (String, bool) {
        let col = t.columns.iter().find(|c| c.name == name);
        let ty = col.map(|c| c.ty.clone()).unwrap_or_default();
        // their isEnum: the column's type resolves to a bound enum symbol
        let is_enum = doc.enums.iter().any(|e| {
            e.name.eq_ignore_ascii_case(ty.trim_matches('"'))
                || format!("{}.{}", e.schema.as_deref().unwrap_or(""), e.name)
                    .eq_ignore_ascii_case(&ty)
        });
        (ty, is_enum)
    };
    let values: Vec<Value> = r
        .rows
        .iter()
        .zip(r.value_spans.iter())
        .map(|(row, spans)| {
            Value::Array(
                row.iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let cname = columns.get(i).map(|s| s.as_str()).unwrap_or("");
                        let (ty, is_enum) = col_type(cname);
                        let mut val = record_value(v, &ty, is_enum);
                        if let Some(sp) = spans.get(i) {
                            if let serde_json::Value::Object(m) = &mut val {
                                m.insert("token".into(), token_json(sp.clone()));
                            }
                        }
                        val
                    })
                    .collect(),
            )
        })
        .collect();
    let mut m = Map::new();
    m.insert("token".into(), token_json(r.span.start..trimmed_end(src, r.span.end)));
    m.insert("schemaName".into(), json!(t.schema));
    m.insert("tableName".into(), json!(t.name));
    m.insert("columns".into(), json!(columns));
    m.insert("values".into(), Value::Array(values));
    if r.example {
        m.insert("example".into(), json!(true));
    }
    Value::Object(m)
}

// ---- their value/typing rules (records/utils/data) ------------------------

fn record_value(v: &RecordValue, col_type: &str, is_enum: bool) -> Value {
    let vt = get_record_value_type(col_type, is_enum);
    match v {
        RecordValue::Null => json!({"value": Value::Null, "type": vt}),
        RecordValue::Expr(e) => json!({"value": e, "type": "expression"}),
        RecordValue::Number(n) => {
            // numeric columns parse; other columns stringify (their
            // tryExtractString falls back to the literal's text)
            if is_integer_type(col_type) || is_float_type(col_type) || is_serial_type(col_type) {
                match n.parse::<f64>() {
                    Ok(num) => json!({"value": json_num(num, n), "type": vt}),
                    Err(_) => json!({"value": n, "type": vt}),
                }
            } else {
                json!({"value": n, "type": vt})
            }
        }
        RecordValue::Bool(b) => {
            if is_boolean_type(col_type) {
                json!({"value": b, "type": vt})
            } else {
                json!({"value": b.to_string(), "type": vt})
            }
        }
        RecordValue::Text(s) => {
            // their isNullish gate: an empty string literal reads as NULL
            // unless the column is a string type
            if s.is_empty() && !is_string_type(col_type) {
                return json!({"value": Value::Null, "type": vt});
            }
            if is_boolean_type(col_type) {
                return match truthy_falsy(s) {
                    Some(b) => json!({"value": b, "type": vt}),
                    None => json!({"value": fallback_text(s), "type": "expression"}),
                };
            }
            if is_date_time_type(col_type) {
                return match normalize_datetime(s) {
                    Some(d) => json!({"value": d, "type": vt}),
                    None => json!({"value": fallback_text(s), "type": "expression"}),
                };
            }
            json!({"value": s, "type": vt})
        }
        RecordValue::Ident(w) => {
            // a bare word is only meaningful as a bool literal or a date;
            // otherwise their extraction falls back to source text with
            // type "expression"
            if is_boolean_type(col_type) {
                return match truthy_falsy(w) {
                    Some(b) => json!({"value": b, "type": vt}),
                    None => json!({"value": w, "type": "expression"}),
                };
            }
            if is_date_time_type(col_type) {
                return match normalize_datetime(w) {
                    Some(d) => json!({"value": d, "type": vt}),
                    None => json!({"value": w, "type": "expression"}),
                };
            }
            json!({"value": w, "type": "expression"})
        }
    }
}

fn fallback_text(s: &str) -> String {
    s.to_string()
}

/// JSON number keeping integer spelling integral (`1` stays `1`).
fn json_num(num: f64, raw: &str) -> Value {
    if raw.contains('.') || raw.contains('e') || raw.contains('E') {
        serde_json::Number::from_f64(num)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    } else if num.fract() == 0.0 && num.abs() < 9e15 {
        json!(num as i64)
    } else {
        serde_json::Number::from_f64(num)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

const TRUTHY: [&str; 5] = ["true", "yes", "y", "t", "1"];
const FALSY: [&str; 5] = ["false", "no", "n", "f", "0"];

fn truthy_falsy(s: &str) -> Option<bool> {
    let lower = s.to_ascii_lowercase();
    if TRUTHY.contains(&lower.as_str()) {
        Some(true)
    } else if FALSY.contains(&lower.as_str()) {
        Some(false)
    } else {
        None
    }
}

// their sqlTypes.ts classifiers, dialect-union form (the records
// interpretation calls them without a dialect)
fn norm_type(t: &str) -> String {
    t.trim().to_ascii_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

macro_rules! type_set {
    ($name:ident, [$($t:expr),* $(,)?]) => {
        fn $name(t: &str) -> bool {
            const SET: &[&str] = &[$($t),*];
            let n = norm_type(t);
            SET.contains(&n.as_str())
        }
    };
}

type_set!(
    is_integer_type,
    ["int", "integer", "smallint", "bigint", "tinyint", "mediumint", "serial", "bigserial", "smallserial"]
);
type_set!(is_float_type, ["decimal", "numeric", "float", "double", "real", "number", "double precision"]);
type_set!(is_boolean_type, ["bool", "boolean", "bit"]);
type_set!(
    is_string_type,
    ["varchar", "char", "text", "tinytext", "mediumtext", "longtext", "string", "character",
     "character varying", "nvarchar", "nchar", "ntext", "varchar2", "nvarchar2"]
);
type_set!(is_serial_type, ["serial", "smallserial", "bigserial"]);
type_set!(
    is_date_time_type,
    ["date", "datetime", "timestamp", "time", "timestamptz", "timestamp with time zone",
     "timestamp without time zone", "timetz", "time with time zone", "time without time zone",
     "datetime2", "smalldatetime", "timestamp with local time zone"]
);

/// their getRecordValueType: enum-bound columns read as strings, then the
/// classifier ladder, then the raw type name.
pub fn get_record_value_type(sql_type: &str, is_enum: bool) -> String {
    if is_enum {
        return "string".to_string();
    }
    let t = sql_type.split('(').next().unwrap_or("");
    if is_integer_type(t) {
        "integer".into()
    } else if is_float_type(t) {
        "real".into()
    } else if is_boolean_type(t) {
        "bool".into()
    } else if is_string_type(t) {
        "string".into()
    } else {
        let normalized = norm_type(t);
        if normalized == "date" {
            "date".into()
        } else if normalized == "time"
            || normalized == "timetz"
            || normalized == "time with time zone"
            || normalized == "time without time zone"
        {
            "time".into()
        } else if is_date_time_type(t) {
            "datetime".into()
        } else {
            sql_type.to_string()
        }
    }
}

/// their tryExtractDateTime: the supported luxon formats plus ISO 8601,
/// normalized to ISO. Offsets pass through; zone conversion is out of
/// scope until a corpus case needs it.
fn normalize_datetime(s: &str) -> Option<String> {
    let v = s.trim();
    // time-only: HH:mm[:ss[.SSS]][Z|±HH[:MM]]
    if let Some(out) = normalize_time(v) {
        return Some(out);
    }
    // dates: yyyy-MM-dd | M/d/yyyy | d MMM yyyy | MMM d, yyyy
    if let Some(d) = normalize_date(v) {
        return Some(d);
    }
    // datetimes: yyyy-MM-dd HH:mm[:ss[.SSS]][offset]
    if let Some(d) = normalize_date_time(v) {
        return Some(d);
    }
    // ISO 8601 fallback: a T-separated datetime or a plain date
    if let Some(d) = normalize_iso(v) {
        return Some(d);
    }
    None
}

fn normalize_time(v: &str) -> Option<String> {
    let (body, _offset) = split_offset(v)?;
    let mut parts = body.split(':');
    let h: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    let rest = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return None;
    }
    if rest.is_empty() {
        return Some(format!("{h:02}:{m:02}:00"));
    }
    let (secs, ms) = match rest.split_once('.') {
        Some((s2, frac)) => (s2.parse::<u32>().ok()?, Some(frac)),
        None => (rest.parse::<u32>().ok()?, None),
    };
    if secs > 59 {
        return None;
    }
    match ms {
        Some(_) => Some(format!("{h:02}:{m:02}:{secs:02}")), // suppressMilliseconds
        None => Some(format!("{h:02}:{m:02}:{secs:02}")),
    }
}

fn split_offset(v: &str) -> Option<(&str, Option<String>)> {
    if let Some(pos) = v.rfind(['+', '-']) {
        if pos > 0 {
            return Some((&v[..pos], Some(v[pos..].to_string())));
        }
    }
    if let Some(stripped) = v.strip_suffix('Z') {
        return Some((stripped, Some("Z".into())));
    }
    Some((v, None))
}

fn normalize_date(v: &str) -> Option<String> {
    // yyyy-MM-dd
    if let Ok(d) = chrono_parse_ymd(v) {
        return Some(d);
    }
    // M/d/yyyy
    let segs: Vec<&str> = v.split('/').collect();
    if segs.len() == 3 {
        let m: u32 = segs[0].parse().ok()?;
        let d: u32 = segs[1].parse().ok()?;
        let y: i32 = segs[2].parse().ok()?;
        if (1..=12).contains(&m) && (1..=31).contains(&d) {
            return Some(format!("{y:04}-{m:02}-{d:02}"));
        }
        return None;
    }
    // d MMM yyyy | MMM d, yyyy
    let months = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let month_of = |w: &str| months.iter().position(|m| w.to_ascii_lowercase().starts_with(m));
    if let Some((d, rest)) = v.split_once(' ') {
        if let Some((mon, y)) = rest.split_once(' ') {
            if let Some(mi) = month_of(mon) {
                let day: u32 = d.trim_end_matches(',').parse().ok()?;
                let year: i32 = y.parse().ok()?;
                return Some(format!("{year:04}-{:02}-{day:02}", mi + 1));
            }
            if let Some(mi) = month_of(d) {
                let day: u32 = mon.trim_end_matches(',').parse().ok()?;
                let year: i32 = y.parse().ok()?;
                return Some(format!("{year:04}-{:02}-{day:02}", mi + 1));
            }
        }
    }
    None
}

fn chrono_parse_ymd(v: &str) -> Result<String, ()> {
    let segs: Vec<&str> = v.split('-').collect();
    if segs.len() != 3 {
        return Err(());
    }
    let y: i32 = segs[0].parse().map_err(|_| ())?;
    let m: u32 = segs[1].parse().map_err(|_| ())?;
    let d: u32 = segs[2].parse().map_err(|_| ())?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(());
    }
    Ok(format!("{y:04}-{m:02}-{d:02}"))
}

fn normalize_date_time(v: &str) -> Option<String> {
    let (date_part, time_part) = v.split_once(' ')?;
    let date = chrono_parse_ymd(date_part).ok()?;
    let time = normalize_time(time_part)?;
    Some(format!("{date}T{time}"))
}

fn normalize_iso(v: &str) -> Option<String> {
    // '2020-01-01T10:00:00[.SSS][Z|offset]' — the T form passes through
    // with the space→T normalization already applied
    let (date_part, rest) = v.split_once('T')?;
    let date = chrono_parse_ymd(date_part).ok()?;
    let (time_body, _offset) = split_offset(rest)?;
    let time = normalize_time(time_body)?;
    Some(format!("{date}T{time}"))
}

// ---- shared helpers -------------------------------------------------------

/// our `ty` string → (schemaName, type_name, args): quotes stripped,
/// args captured with inner spaces removed (`decimal(10, 2)` →
/// `decimal(10,2)` / `"10,2"`; `int[]` keeps its brackets).
fn split_type(ty: &str) -> (Option<String>, String, Option<String>) {
    if ty.starts_with('"') {
        let mut segs: Vec<String> = Vec::new();
        let mut rest = ty;
        loop {
            if let Some(r) = rest.strip_prefix('"') {
                let end = r.find('"').unwrap_or(r.len());
                segs.push(r[..end].to_string());
                rest = &r[end + 1..];
            } else {
                break;
            }
            match rest.strip_prefix('.') {
                Some(r2) => rest = r2,
                None => break,
            }
        }
        let args = rest
            .strip_prefix('(')
            .map(|inner| inner.trim_end_matches(')').replace(' ', ""));
        let name = segs.pop().unwrap_or_default();
        let schema = if segs.is_empty() {
            None
        } else {
            Some(segs.join("."))
        };
        let type_name = match &args {
            Some(a) => format!("{name}({a})"),
            None => name,
        };
        return (schema, type_name, args);
    }
    let (word, args_raw) = match ty.find('(') {
        Some(i) => (&ty[..i], Some(ty[i + 1..].trim_end_matches(')').replace(' ', ""))),
        None => (ty, None),
    };
    let (schema, name) = match word.split_once('.') {
        Some((s, n)) => (Some(s.to_string()), n.to_string()),
        None => (None, word.to_string()),
    };
    let type_name = match &args_raw {
        Some(a) => format!("{name}({a})"),
        None => name,
    };
    (schema, type_name, args_raw)
}

/// their formatTypedValue-ish default typing: quoted → string (quotes
/// off), backticks → expression, true/false/null → boolean (the null
/// literal types as boolean "null" — probe-verified), numbers stay JSON
/// numbers.
fn typed_value(raw: &str) -> Value {
    if let Some(inner) = raw.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        if raw.len() >= 2 {
            return json!({"value": inner, "type": "string"});
        }
    }
    if let Some(inner) = raw.strip_prefix('`').and_then(|r| r.strip_suffix('`')) {
        if raw.len() >= 2 {
            return json!({"value": inner, "type": "expression"});
        }
    }
    match raw {
        "true" | "false" | "null" => json!({"value": raw, "type": "boolean"}),
        _ => match raw.parse::<f64>() {
            Ok(num) => json!({"value": json_num(num, raw), "type": "number"}),
            Err(_) => json!({"value": raw, "type": "expression"}),
        },
    }
}

/// their CustomMetadataValueType: color for #hex, else the literal kind.
fn custom_json(custom: &[(String, String)]) -> Value {
    let mut m = Map::new();
    for (k, v) in custom {
        let ty = if v.starts_with('#') {
            "color"
        } else if v.starts_with('\'') {
            "string"
        } else if v == "true" || v == "false" {
            "bool"
        } else if v.parse::<f64>().is_ok() {
            "number"
        } else {
            "string"
        };
        let value = if ty == "string" {
            json!(v.trim_matches('\''))
        } else if ty == "number" {
            json!(v.parse::<f64>().unwrap_or(0.0))
        } else if ty == "bool" {
            json!(v == "true")
        } else {
            json!(v)
        };
        m.insert(k.clone(), json!({"value": value, "type": ty}));
    }
    Value::Object(m)
}

fn card_str(c: super::validate::Card) -> &'static str {
    use super::validate::Card::*;
    match c {
        One => "1",
        Maybe => "0..1",
        Some => "*",
        Many => "0..*",
    }
}
