//! Ad-hoc SQL execution with a safety row cap.
//!
//! `run_query` wraps a user statement in a defensive `LIMIT` so a careless
//! `SELECT *` over a 50GB table cannot OOM the frontend. The cap is applied
//! by wrapping the query as a subquery; DuckDB's optimizer folds the wrapper.

use std::time::Instant;

use duckdb::types::Value as DuckValue;

use crate::model::SqlResult;
use crate::duckdb::QUERY_TIMEOUT_SECS;

/// Run a SELECT and return a row-capped [`SqlResult`]. `cap` of `None` means
/// no cap (the caller must opt in explicitly).
pub fn run_query(conn: &duckdb::Connection, sql: &str, cap: Option<usize>) -> Result<SqlResult, String> {
    let start = Instant::now();
    let interrupt_handle = conn.interrupt_handle();
    let timeout_secs = QUERY_TIMEOUT_SECS;

    // 软超时：起一个后台线程，超时后 interrupt 查询。
    let timer_cancel = if timeout_secs > 0 {
        let handle = interrupt_handle.clone();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            if rx.recv_timeout(std::time::Duration::from_secs(timeout_secs)).is_err() {
                tracing::warn!(
                    category = "query",
                    "query exceeded {}s soft limit — interrupt fired",
                    timeout_secs,
                );
                handle.interrupt();
            }
        });
        Some(tx)
    } else {
        None
    };

    // 把用户 SQL 包成子查询再加 LIMIT（若用户自己的 LIMIT ≤ cap 则透传）。
    let inner = sql.trim().trim_end_matches(';');
    let existing_limit = parse_trailing_limit(inner);
    let wrapped = match (cap, existing_limit) {
        (Some(cap_val), Some(limit_val)) if limit_val <= cap_val as u64 => inner.to_string(),
        (Some(cap_val), _) => format!("SELECT * FROM ({inner}) AS _q LIMIT {cap_val}"),
        (None, _) => inner.to_string(),
    };

    let query_res = (|| -> Result<SqlResult, String> {
        let mut stmt = conn.prepare(&wrapped).map_err(|e| e.to_string())?;

        let mut rows_out: Vec<Vec<serde_json::Value>> = Vec::new();
        if let Some(cap_val) = cap {
            rows_out.reserve(cap_val);
        }
        // DuckDB 的 schema() 在 query 执行后才可用，所以列数从第一行探测。
        let mut ncol: usize = 0;
        {
            let mut iter = stmt.query([]).map_err(|e| e.to_string())?;
            while let Some(row) = iter.next().map_err(|e| e.to_string())? {
                if ncol == 0 {
                    // 第一行：探测列数，边探测边收集。
                    let mut out = Vec::new();
                    let mut idx = 0;
                    while let Ok(val) = row.get::<usize, DuckValue>(idx) {
                        out.push(duck_value_to_json(val));
                        idx += 1;
                    }
                    ncol = idx;
                    rows_out.push(out);
                } else {
                    let mut out = Vec::with_capacity(ncol);
                    for idx in 0..ncol {
                        let val: DuckValue = row.get(idx).map_err(|e| e.to_string())?;
                        out.push(duck_value_to_json(val));
                    }
                    rows_out.push(out);
                }
            }
        } // iter dropped here, releasing the borrow on stmt

        let schema = stmt.schema();
        let column_names: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
        let column_types: Vec<String> = schema.fields().iter().map(|f| format!("{}", f.data_type())).collect();

        let truncated = cap.map_or(false, |n| rows_out.len() >= n);
        let row_count = rows_out.len();

        Ok(SqlResult {
            columns: column_names,
            column_types,
            rows: rows_out,
            row_count,
            truncated,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    })();

    if let Some(tx) = timer_cancel {
        let _ = tx.send(());
    }

    match query_res {
        Ok(res) => Ok(res),
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("interrupted") || err_msg.contains("Interrupt") {
                Err(format!(
                    "查询超时已被自动中断（当前限制为 {} 秒）。",
                    timeout_secs
                ))
            } else {
                Err(e)
            }
        }
    }
}

/// Map a DuckDB runtime value to JSON, preserving nulls and numeric precision.
fn duck_value_to_json(v: DuckValue) -> serde_json::Value {
    match v {
        DuckValue::Null => serde_json::Value::Null,
        DuckValue::Boolean(b) => serde_json::Value::Bool(b),
        DuckValue::TinyInt(i) => num_i64(i as i64),
        DuckValue::SmallInt(i) => num_i64(i as i64),
        DuckValue::Int(i) => num_i64(i as i64),
        DuckValue::BigInt(i) => num_i64(i),
        // HugeInt overflows f64/i64; stringify to preserve precision.
        DuckValue::HugeInt(i) => serde_json::Value::String(i.to_string()),
        DuckValue::UTinyInt(u) => num_u64(u as u64),
        DuckValue::USmallInt(u) => num_u64(u as u64),
        DuckValue::UInt(u) => num_u64(u as u64),
        DuckValue::UBigInt(u) => num_u64(u),
        DuckValue::Float(f) => serde_json::Number::from_f64(f as f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        DuckValue::Double(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        DuckValue::Decimal(d) => serde_json::Value::String(d.to_string()),
        DuckValue::Timestamp(unit, micros) => serde_json::Value::String(format_ts(unit, micros)),
        DuckValue::Text(s) => serde_json::Value::String(s),
        DuckValue::Blob(b) => {
            use std::fmt::Write as _;
            let mut hex = String::with_capacity(b.len() * 2);
            for byte in b.iter() {
                let _ = write!(hex, "{:02x}", byte);
            }
            serde_json::Value::String(hex)
        }
        DuckValue::Date32(days) => serde_json::Value::String(format_date(days)),
        DuckValue::Time64(unit, v) => serde_json::Value::String(format_time(unit, v)),
        DuckValue::Interval { months, days, nanos } => {
            serde_json::Value::String(format!("{months} months {days} days {nanos} ns"))
        }
        DuckValue::List(items) => serde_json::Value::Array(items.into_iter().map(duck_value_to_json).collect()),
        DuckValue::Enum(s) => serde_json::Value::String(s),
        DuckValue::Struct(map) => {
            let mut obj = serde_json::Map::new();
            for (k, val) in map.iter().cloned() {
                obj.insert(k, duck_value_to_json(val));
            }
            serde_json::Value::Object(obj)
        }
        DuckValue::Array(items) => serde_json::Value::Array(items.into_iter().map(duck_value_to_json).collect()),
        DuckValue::Map(map) => {
            let mut arr = Vec::new();
            for (k, val) in map.iter().cloned() {
                let mut entry = serde_json::Map::new();
                entry.insert("key".to_string(), duck_value_to_json(k));
                entry.insert("value".to_string(), duck_value_to_json(val));
                arr.push(serde_json::Value::Object(entry));
            }
            serde_json::Value::Array(arr)
        }
        DuckValue::Union(inner) => duck_value_to_json(*inner),
        _ => serde_json::Value::Null,
    }
}

fn num_i64(i: i64) -> serde_json::Value {
    serde_json::Number::from(i).into()
}
fn num_u64(u: u64) -> serde_json::Value {
    serde_json::Number::from(u).into()
}

// --- temporal formatting ----------------------------------------------------

use duckdb::types::TimeUnit;

fn format_ts(unit: TimeUnit, raw: i64) -> String {
    let _ = unit;
    let micros = raw;
    let secs = micros.div_euclid(1_000_000);
    let rem_us = micros.rem_euclid(1_000_000);
    civil_from_secs(secs, rem_us)
}

fn format_time(unit: TimeUnit, raw: i64) -> String {
    let _ = unit;
    let micros = raw.rem_euclid(86_400 * 1_000_000);
    let tod = micros / 1_000_000;
    let h = tod / 3600;
    let m = (tod % 3600) / 60;
    let s = tod % 60;
    let us = micros % 1_000_000;
    format!("{h:02}:{m:02}:{s:02}.{us:06}")
}

fn format_date(days_since_epoch: i32) -> String {
    let z = days_since_epoch as i64 + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02}")
}

fn civil_from_secs(secs: i64, rem_us: i64) -> String {
    let (days, tod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let h = tod / 3600;
    let m = (tod % 3600) / 60;
    let s = tod % 60;
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02} {h:02}:{m:02}:{s:02}.{rem_us:06}")
}

/// Parse the trailing limit value from a SQL query string if present.
/// Supports both `LIMIT <n>` and `LIMIT <n> OFFSET <m>` at the end of the query.
fn parse_trailing_limit(sql: &str) -> Option<u64> {
    let sql_trimmed = sql.trim().trim_end_matches(';').trim();
    let tokens: Vec<&str> = sql_trimmed.split_whitespace().collect();
    let len = tokens.len();
    if len >= 2 {
        let last_token = tokens[len - 1];
        let prev_token = tokens[len - 2];
        if prev_token.eq_ignore_ascii_case("limit") {
            return last_token.parse::<u64>().ok();
        }
        if len >= 4 {
            let offset_key = tokens[len - 2];
            let limit_val = tokens[len - 3];
            let limit_key = tokens[len - 4];
            if limit_key.eq_ignore_ascii_case("limit") && offset_key.eq_ignore_ascii_case("offset") {
                return limit_val.parse::<u64>().ok();
            }
        }
    }
    None
}
