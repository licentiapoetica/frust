//! Postgres access.
//!
//! [`Sql`] is a composable query builder that plays the role of postgres.js tagged templates:
//! text is pushed with [`Sql::push`], values are always sent as bound parameters with
//! [`Sql::bind`], and fragments can be nested with [`Sql::append`] (the `${db`...`}` pattern).
//! Raw SQL from code (never from users) can be spliced with [`Sql::push`] — the equivalent of
//! `db.unsafe(...)`.
//!
//! Untyped results come back as [`Row`] (a JSON object) using the same value mapping as
//! postgres.js, so data handed to templates and JSON responses keeps its old shape:
//! int2/int4/float → number, int8/numeric → string, timestamps → ISO-8601 string,
//! json/jsonb → parsed JSON, arrays → arrays.

use anyhow::Result;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use serde_json::{Map, Value};
use sqlx::postgres::{PgArguments, PgPoolOptions, PgRow};
use sqlx::{Arguments, Column, PgPool, Postgres, Row as _, TypeInfo, ValueRef};

pub type Row = Map<String, Value>;

pub async fn connect(cfg: &crate::config::Config) -> Result<PgPool> {
    let (host, port, user, password, database) = cfg.database_url_parts();
    let max = cfg.int("sql.max").and_then(|m| u32::try_from(m).ok()).unwrap_or(50);
    let opts = sqlx::postgres::PgConnectOptions::new()
        .host(&host)
        .port(port)
        .username(&user)
        .password(&password)
        .database(&database)
        .application_name("f0ckm");
    let pool = PgPoolOptions::new()
        .max_connections(max)
        .acquire_timeout(std::time::Duration::from_secs(30))
        .connect_lazy_with(opts);
    Ok(pool)
}

/// A bound parameter.
#[derive(Debug, Clone)]
pub enum Bind {
    Null,
    Bool(bool),
    I16(i16),
    I32(i32),
    I64(i64),
    F64(f64),
    Text(String),
    Json(Value),
    Timestamptz(DateTime<Utc>),
    I32Array(Vec<i32>),
    I64Array(Vec<i64>),
    TextArray(Vec<String>),
    NullText,
    NullI32,
    NullI64,
}

macro_rules! bind_from {
    ($t:ty, $v:ident => $e:expr) => {
        impl From<$t> for Bind {
            fn from($v: $t) -> Bind {
                $e
            }
        }
    };
}
bind_from!(bool, v => Bind::Bool(v));
bind_from!(i16, v => Bind::I16(v));
bind_from!(i32, v => Bind::I32(v));
bind_from!(i64, v => Bind::I64(v));
bind_from!(u32, v => Bind::I64(v as i64));
bind_from!(usize, v => Bind::I64(v as i64));
bind_from!(f64, v => Bind::F64(v));
bind_from!(String, v => Bind::Text(v));
bind_from!(&str, v => Bind::Text(v.to_string()));
bind_from!(&String, v => Bind::Text(v.clone()));
bind_from!(Value, v => Bind::Json(v));
bind_from!(DateTime<Utc>, v => Bind::Timestamptz(v));
bind_from!(Vec<i32>, v => Bind::I32Array(v));
bind_from!(Vec<i64>, v => Bind::I64Array(v));
bind_from!(Vec<String>, v => Bind::TextArray(v));
bind_from!(&[i32], v => Bind::I32Array(v.to_vec()));
bind_from!(&[i64], v => Bind::I64Array(v.to_vec()));
bind_from!(&[String], v => Bind::TextArray(v.to_vec()));
bind_from!(Option<String>, v => v.map(Bind::Text).unwrap_or(Bind::NullText));
bind_from!(Option<&str>, v => v.map(|s| Bind::Text(s.to_string())).unwrap_or(Bind::NullText));
bind_from!(Option<i32>, v => v.map(Bind::I32).unwrap_or(Bind::NullI32));
bind_from!(Option<i64>, v => v.map(Bind::I64).unwrap_or(Bind::NullI64));
bind_from!(Option<bool>, v => v.map(Bind::Bool).unwrap_or(Bind::Null));

const PLACEHOLDER: char = '\u{1}';

/// Composable SQL with bound parameters.
#[derive(Debug, Clone, Default)]
pub struct Sql {
    text: String,
    binds: Vec<Bind>,
}

impl Sql {
    pub fn new(text: &str) -> Sql {
        Sql { text: text.to_string(), binds: Vec::new() }
    }

    pub fn empty() -> Sql {
        Sql::default()
    }

    /// Append trusted SQL text. Never pass user input here.
    pub fn push(&mut self, text: &str) -> &mut Sql {
        self.text.push_str(text);
        self
    }

    /// Append a bound parameter (`$n`).
    pub fn bind(&mut self, v: impl Into<Bind>) -> &mut Sql {
        self.text.push(PLACEHOLDER);
        self.binds.push(v.into());
        self
    }

    /// Append another fragment (with its parameters).
    pub fn append(&mut self, other: &Sql) -> &mut Sql {
        self.text.push_str(&other.text);
        self.binds.extend(other.binds.iter().cloned());
        self
    }

    /// Append `($1, $2, ...)` — the `in ${db(list)}` pattern.
    pub fn bind_list<T: Into<Bind> + Clone>(&mut self, items: &[T]) -> &mut Sql {
        self.text.push('(');
        for (i, v) in items.iter().enumerate() {
            if i > 0 {
                self.text.push_str(", ");
            }
            self.bind(v.clone());
        }
        self.text.push(')');
        self
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// Final SQL text with `$n` placeholders.
    pub fn sql(&self) -> String {
        let mut out = String::with_capacity(self.text.len() + self.binds.len() * 3);
        let mut n = 0;
        for c in self.text.chars() {
            if c == PLACEHOLDER {
                n += 1;
                out.push('$');
                out.push_str(&n.to_string());
            } else {
                out.push(c);
            }
        }
        out
    }

    fn args(&self) -> Result<PgArguments> {
        let mut a = PgArguments::default();
        for b in &self.binds {
            let r = match b {
                Bind::Null => a.add(None::<bool>),
                Bind::NullText => a.add(None::<String>),
                Bind::NullI32 => a.add(None::<i32>),
                Bind::NullI64 => a.add(None::<i64>),
                Bind::Bool(v) => a.add(*v),
                Bind::I16(v) => a.add(*v),
                Bind::I32(v) => a.add(*v),
                Bind::I64(v) => a.add(*v),
                Bind::F64(v) => a.add(*v),
                Bind::Text(v) => a.add(v.clone()),
                Bind::Json(v) => a.add(sqlx::types::Json(v.clone())),
                Bind::Timestamptz(v) => a.add(*v),
                Bind::I32Array(v) => a.add(v.clone()),
                Bind::I64Array(v) => a.add(v.clone()),
                Bind::TextArray(v) => a.add(v.clone()),
            };
            r.map_err(|e| anyhow::anyhow!("bind error: {e}"))?;
        }
        Ok(a)
    }

    /// Run and return rows as JSON objects.
    pub async fn fetch_all<'e, E: sqlx::PgExecutor<'e>>(&self, db: E) -> Result<Vec<Row>> {
        let sql = self.sql();
        let rows = sqlx::query_with(&sql, self.args()?)
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!("{e}\n  query: {}", compact(&sql)))?;
        Ok(rows.iter().map(row_to_json).collect())
    }

    pub async fn fetch_optional<'e, E: sqlx::PgExecutor<'e>>(&self, db: E) -> Result<Option<Row>> {
        let sql = self.sql();
        let row = sqlx::query_with(&sql, self.args()?)
            .fetch_optional(db)
            .await
            .map_err(|e| anyhow::anyhow!("{e}\n  query: {}", compact(&sql)))?;
        Ok(row.as_ref().map(row_to_json))
    }

    /// Run and decode into a typed struct.
    pub async fn fetch_as<'e, T, E>(&self, db: E) -> Result<Vec<T>>
    where
        T: for<'r> sqlx::FromRow<'r, PgRow> + Send + Unpin,
        E: sqlx::PgExecutor<'e>,
    {
        let sql = self.sql();
        sqlx::query_as_with::<_, T, _>(&sql, self.args()?)
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!("{e}\n  query: {}", compact(&sql)))
    }

    pub async fn fetch_one_as<'e, T, E>(&self, db: E) -> Result<Option<T>>
    where
        T: for<'r> sqlx::FromRow<'r, PgRow> + Send + Unpin,
        E: sqlx::PgExecutor<'e>,
    {
        let sql = self.sql();
        sqlx::query_as_with::<_, T, _>(&sql, self.args()?)
            .fetch_optional(db)
            .await
            .map_err(|e| anyhow::anyhow!("{e}\n  query: {}", compact(&sql)))
    }

    /// Single scalar (first column of the first row).
    pub async fn fetch_scalar<'e, T, E>(&self, db: E) -> Result<Option<T>>
    where
        T: for<'r> sqlx::Decode<'r, Postgres> + sqlx::Type<Postgres> + Send + Unpin,
        E: sqlx::PgExecutor<'e>,
    {
        let sql = self.sql();
        let row = sqlx::query_with(&sql, self.args()?)
            .fetch_optional(db)
            .await
            .map_err(|e| anyhow::anyhow!("{e}\n  query: {}", compact(&sql)))?;
        match row {
            Some(r) => Ok(r.try_get::<Option<T>, _>(0)?),
            None => Ok(None),
        }
    }

    /// First column of every row.
    pub async fn fetch_scalar_list<'e, T, E>(&self, db: E) -> Result<Vec<T>>
    where
        T: for<'r> sqlx::Decode<'r, Postgres> + sqlx::Type<Postgres> + Send + Unpin,
        E: sqlx::PgExecutor<'e>,
    {
        let sql = self.sql();
        let rows = sqlx::query_with(&sql, self.args()?)
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!("{e}\n  query: {}", compact(&sql)))?;
        rows.iter().map(|r| r.try_get::<T, _>(0).map_err(Into::into)).collect()
    }

    /// Execute; returns affected row count.
    pub async fn execute<'e, E: sqlx::PgExecutor<'e>>(&self, db: E) -> Result<u64> {
        let sql = self.sql();
        let r = sqlx::query_with(&sql, self.args()?)
            .execute(db)
            .await
            .map_err(|e| anyhow::anyhow!("{e}\n  query: {}", compact(&sql)))?;
        Ok(r.rows_affected())
    }
}

/// `sql!("select ... where id = ", id, " and x = ", x)` — string literals are SQL text,
/// everything else is bound. Nested `Sql` fragments go through `.append()`.
#[macro_export]
macro_rules! sql {
    ($($part:tt)*) => {{
        #[allow(unused_mut)]
        let mut q = $crate::db::Sql::empty();
        $crate::sql_parts!(q; $($part)*);
        q
    }};
}

#[macro_export]
#[doc(hidden)]
macro_rules! sql_parts {
    ($q:ident;) => {};
    ($q:ident; $lit:literal $(, $($rest:tt)*)?) => {
        $q.push($lit);
        $( $crate::sql_parts!($q; $($rest)*); )?
    };
    ($q:ident; @ $frag:expr $(, $($rest:tt)*)?) => {
        $q.append(&$frag);
        $( $crate::sql_parts!($q; $($rest)*); )?
    };
    ($q:ident; $val:expr $(, $($rest:tt)*)?) => {
        $q.bind($val);
        $( $crate::sql_parts!($q; $($rest)*); )?
    };
}

fn compact(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ts_to_json(dt: DateTime<Utc>) -> Value {
    Value::String(dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// Convert a row to a JSON object using postgres.js value conventions.
pub fn row_to_json(row: &PgRow) -> Row {
    let mut map = Map::with_capacity(row.columns().len());
    for col in row.columns() {
        let idx = col.ordinal();
        let name = col.name().to_string();
        let is_null = row.try_get_raw(idx).map(|v| v.is_null()).unwrap_or(true);
        if is_null {
            map.insert(name, Value::Null);
            continue;
        }
        let ty = col.type_info().name().to_ascii_uppercase();
        let v = decode(row, idx, &ty).unwrap_or_else(|| {
            tracing::warn!("db: cannot decode column {name} of type {ty}");
            Value::Null
        });
        map.insert(name, v);
    }
    map
}

fn decode(row: &PgRow, idx: usize, ty: &str) -> Option<Value> {
    macro_rules! get {
        ($t:ty) => {
            row.try_get::<$t, _>(idx).ok()
        };
    }
    Some(match ty {
        "BOOL" => Value::Bool(get!(bool)?),
        "INT2" => Value::from(get!(i16)?),
        "INT4" => Value::from(get!(i32)?),
        "OID" => Value::from(get!(sqlx::postgres::types::Oid)?.0),
        "INT8" => Value::String(get!(i64)?.to_string()),
        "FLOAT4" => serde_json::Number::from_f64(get!(f32)? as f64).map(Value::Number).unwrap_or(Value::Null),
        "FLOAT8" => serde_json::Number::from_f64(get!(f64)?).map(Value::Number).unwrap_or(Value::Null),
        "NUMERIC" => Value::String(numeric_text(row, idx)?),
        "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" | "CHAR" | "CITEXT" | "UNKNOWN" | "XML" => Value::String(get!(String)?),
        "UUID" => Value::String(get!(uuid::Uuid)?.to_string()),
        "JSON" | "JSONB" => get!(sqlx::types::Json<Value>)?.0,
        "TIMESTAMPTZ" => ts_to_json(get!(DateTime<Utc>)?),
        "TIMESTAMP" => ts_to_json(get!(NaiveDateTime)?.and_utc()),
        "DATE" => ts_to_json(get!(NaiveDate)?.and_hms_opt(0, 0, 0)?.and_utc()),
        "TIME" => Value::String(get!(NaiveTime)?.to_string()),
        "BYTEA" => Value::String(hex::encode(get!(Vec<u8>)?)),
        "INT2[]" | "_INT2" => Value::from(get!(Vec<i16>)?),
        "INT4[]" | "_INT4" => Value::from(get!(Vec<i32>)?),
        "INT8[]" | "_INT8" => Value::from(get!(Vec<i64>)?.into_iter().map(|n| n.to_string()).collect::<Vec<_>>()),
        "FLOAT8[]" | "_FLOAT8" => Value::from(get!(Vec<f64>)?),
        "BOOL[]" | "_BOOL" => Value::from(get!(Vec<bool>)?),
        "TEXT[]" | "_TEXT" | "VARCHAR[]" | "_VARCHAR" | "BPCHAR[]" | "_BPCHAR" | "NAME[]" | "_NAME" => Value::from(get!(Vec<String>)?),
        "JSONB[]" | "_JSONB" | "JSON[]" | "_JSON" => Value::from(get!(Vec<sqlx::types::Json<Value>>)?.into_iter().map(|j| j.0).collect::<Vec<_>>()),
        "TIMESTAMPTZ[]" | "_TIMESTAMPTZ" => Value::from(get!(Vec<DateTime<Utc>>)?.into_iter().map(ts_to_json).collect::<Vec<_>>()),
        "VOID" => Value::Null,
        _ => Value::String(get!(String)?),
    })
}

/// NUMERIC as its decimal text (with its display scale), like postgres.js.
fn numeric_text(row: &PgRow, idx: usize) -> Option<String> {
    let raw = row.try_get_raw(idx).ok()?;
    let b = raw.as_bytes().ok()?;
    decode_numeric(b)
}

/// Postgres binary NUMERIC: ndigits, weight, sign, dscale, then base-10000 digits.
fn decode_numeric(b: &[u8]) -> Option<String> {
    if b.len() < 8 {
        return None;
    }
    let rd = |i: usize| i16::from_be_bytes([b[i], b[i + 1]]);
    let ndigits = rd(0) as usize;
    let weight = rd(2) as i32;
    let sign = u16::from_be_bytes([b[4], b[5]]);
    let dscale = rd(6).max(0) as usize;
    if sign == 0xC000 {
        return Some("NaN".into());
    }
    if b.len() < 8 + ndigits * 2 {
        return None;
    }
    let digits: Vec<i16> = (0..ndigits).map(|i| rd(8 + i * 2)).collect();
    // Integer part: groups with exponent >= 0
    let mut int = String::new();
    if weight < 0 {
        int.push('0');
    } else {
        for g in 0..=weight {
            let d = digits.get(g as usize).copied().unwrap_or(0);
            if g == 0 { int.push_str(&d.to_string()) } else { int.push_str(&format!("{d:04}")) }
        }
    }
    let mut out = String::new();
    if sign == 0x4000 {
        out.push('-');
    }
    out.push_str(&int);
    if dscale > 0 {
        let mut frac = String::new();
        let mut g = weight + 1;
        while frac.len() < dscale {
            let d = if g < 0 { 0 } else { digits.get(g as usize).copied().unwrap_or(0) };
            frac.push_str(&format!("{d:04}"));
            g += 1;
        }
        frac.truncate(dscale);
        out.push('.');
        out.push_str(&frac);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(ndigits: i16, weight: i16, sign: u16, dscale: i16, digits: &[i16]) -> Vec<u8> {
        let mut v = Vec::new();
        for x in [ndigits, weight] {
            v.extend_from_slice(&x.to_be_bytes());
        }
        v.extend_from_slice(&sign.to_be_bytes());
        v.extend_from_slice(&dscale.to_be_bytes());
        for d in digits {
            v.extend_from_slice(&d.to_be_bytes());
        }
        v
    }

    #[test]
    fn numeric_decoding() {
        assert_eq!(decode_numeric(&enc(1, -1, 0, 2, &[5000])).unwrap(), "0.50");
        assert_eq!(decode_numeric(&enc(2, 0, 0, 2, &[1, 2500])).unwrap(), "1.25");
        assert_eq!(decode_numeric(&enc(2, 1, 0x4000, 0, &[12, 3456])).unwrap(), "-123456");
        assert_eq!(decode_numeric(&enc(0, 0, 0, 0, &[])).unwrap(), "0");
        assert_eq!(decode_numeric(&enc(1, -2, 0, 8, &[12])).unwrap(), "0.00000012");
    }
}

/// Accessors for JSON rows with JavaScript-style coercion.
pub trait RowExt {
    fn val(&self, key: &str) -> &Value;
    fn str(&self, key: &str) -> Option<&str>;
    fn string(&self, key: &str) -> String;
    fn i64(&self, key: &str) -> Option<i64>;
    fn f64(&self, key: &str) -> Option<f64>;
    fn bool(&self, key: &str) -> bool;
}

impl RowExt for Row {
    fn val(&self, key: &str) -> &Value {
        static NULL: Value = Value::Null;
        self.get(key).unwrap_or(&NULL)
    }
    fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }
    fn string(&self, key: &str) -> String {
        match self.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(v) => v.to_string(),
        }
    }
    fn i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(crate::config::as_int)
    }
    fn f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(crate::config::as_f64)
    }
    fn bool(&self, key: &str) -> bool {
        self.get(key).map(crate::config::truthy).unwrap_or(false)
    }
}
