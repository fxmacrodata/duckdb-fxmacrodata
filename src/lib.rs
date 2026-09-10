//! DuckDB extension exposing FXMacroData as table functions.
//!
//! DuckDB can already read the API with `read_json_auto`, but every query then
//! carries a URL, an `unnest`, and string columns where dates and timestamps
//! belong. These table functions give the same data a name, typed columns and
//! an API key that travels in a header instead of a URL.

/// Thin client for the public FXMacroData REST API.
mod api {
    //! Thin client for the public FXMacroData REST API.
    //!
    //! Nothing in here knows about DuckDB. It builds a URL, sends one GET and hands
    //! back parsed JSON, so the table functions in `lib.rs` stay about columns.

    use serde_json::Value;
    use std::env;
    use std::error::Error;
    use std::time::Duration;

    pub const DEFAULT_BASE_URL: &str = "https://api.fxmacrodata.com/v1";
    const TIMEOUT_SECS: u64 = 30;

    /// Where the API lives. `FXMACRODATA_BASE_URL` overrides it for testing against
    /// a different deployment.
    pub fn base_url() -> String {
        env::var("FXMACRODATA_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }

    /// The API key, if one is configured.
    ///
    /// USD data is public, so the absence of a key is a normal setup rather than an
    /// error. `FXMACRODATA_API_KEY` is the documented name; `FXMD_API_KEY` is
    /// accepted as an alias so a shell already set up for the REST API works here.
    fn api_key() -> Option<String> {
        for name in ["FXMACRODATA_API_KEY", "FXMD_API_KEY"] {
            if let Ok(value) = env::var(name) {
                let value = value.trim().to_string();
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
        None
    }

    /// GET one endpoint and return the parsed JSON body.
    ///
    /// `params` are appended as a query string; entries with an empty value are
    /// dropped so callers can pass optional arguments through unconditionally.
    pub fn get(endpoint: &str, params: &[(&str, String)]) -> Result<Value, Box<dyn Error>> {
        let mut url = format!("{}/{}", base_url(), endpoint.trim_start_matches('/'));

        let query: Vec<String> = params
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{}={}", k, urlencode(v)))
            .collect();
        if !query.is_empty() {
            url.push('?');
            url.push_str(&query.join("&"));
        }

        // The base URL is settable through an environment variable, so pin the
        // scheme rather than letting a stray value reach the transport.
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(format!("FXMacroData base URL must be http or https, got {url}").into());
        }

        let mut request = ureq::agent()
            .get(&url)
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .set("Accept", "application/json");

        if let Some(key) = api_key() {
            // Sent as a header rather than a query parameter so the key is not
            // written into proxy logs, server access logs or a DuckDB query plan.
            request = request.set("X-API-Key", &key);
        }

        match request.call() {
            Ok(response) => Ok(serde_json::from_reader(response.into_reader())?),
            Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => Err(format!(
                "FXMacroData denied {endpoint} (HTTP {code}). That data needs an API key: set \
                 FXMACRODATA_API_KEY. USD macro data is available without one."
            )
            .into()),
            Err(ureq::Error::Status(code, _)) => {
                Err(format!("FXMacroData request to {endpoint} failed with HTTP {code}.").into())
            }
            Err(err) => Err(format!("FXMacroData request to {endpoint} failed: {err}").into()),
        }
    }

    /// Percent-encode a query parameter value.
    ///
    /// Indicator slugs and currency codes are `[a-z0-9_]`, but a caller can pass an
    /// arbitrary string and it should not be able to inject a second parameter.
    fn urlencode(value: &str) -> String {
        let mut out = String::with_capacity(value.len());
        for byte in value.as_bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(*byte as char)
                }
                _ => out.push_str(&format!("%{byte:02X}")),
            }
        }
        out
    }

    /// Pull the row array out of a response body.
    ///
    /// Most endpoints wrap rows in `data`; a few return a bare array.
    pub fn rows(payload: &Value) -> Vec<Value> {
        match payload {
            Value::Array(items) => items.clone(),
            Value::Object(map) => match map.get("data") {
                Some(Value::Array(items)) => items.clone(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Read a nested string field, e.g. `["latest", "source"]`.
    pub fn str_at(row: &Value, path: &[&str]) -> Option<String> {
        let mut node = row;
        for key in path {
            node = node.get(*key)?;
        }
        node.as_str().map(|s| s.to_string())
    }

    /// Read a nested numeric field as f64.
    pub fn f64_at(row: &Value, path: &[&str]) -> Option<f64> {
        let mut node = row;
        for key in path {
            node = node.get(*key)?;
        }
        node.as_f64()
    }

    /// Read a nested integer field as i64.
    pub fn i64_at(row: &Value, path: &[&str]) -> Option<i64> {
        let mut node = row;
        for key in path {
            node = node.get(*key)?;
        }
        node.as_i64()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        #[test]
        fn query_values_are_encoded() {
            // A caller-supplied value must not be able to open a second parameter.
            assert_eq!(urlencode("usd"), "usd");
            assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
        }

        #[test]
        fn rows_handles_both_wrapped_and_bare_arrays() {
            assert_eq!(rows(&json!({"data": [1, 2]})).len(), 2);
            assert_eq!(rows(&json!([1, 2, 3])).len(), 3);
            assert_eq!(rows(&json!({"count": 0})).len(), 0);
            assert_eq!(rows(&json!("nope")).len(), 0);
        }

        #[test]
        fn nested_accessors_walk_paths_and_tolerate_gaps() {
            let row = json!({"latest": {"val": 3.4, "source": "BLS", "announcement_datetime": 17}});
            assert_eq!(f64_at(&row, &["latest", "val"]), Some(3.4));
            assert_eq!(str_at(&row, &["latest", "source"]), Some("BLS".to_string()));
            assert_eq!(i64_at(&row, &["latest", "announcement_datetime"]), Some(17));
            assert_eq!(f64_at(&row, &["latest", "missing"]), None);
            assert_eq!(str_at(&row, &["absent", "source"]), None);
        }

        #[test]
        fn a_null_value_reads_as_absent_not_zero() {
            // val is documented as anyOf[number, null]; treating null as 0.0 would
            // silently turn "no figure" into a real-looking reading.
            let row = json!({"val": serde_json::Value::Null});
            assert_eq!(f64_at(&row, &["val"]), None);
        }
    }
}


use duckdb::{
    Connection, Result,
    core::{DataChunkHandle, Inserter, LogicalTypeHandle, LogicalTypeId},
    duckdb_entrypoint_c_api,
    vtab::{BindInfo, InitInfo, TableFunctionInfo, VTab},
};
use serde_json::Value;

/// DuckDB's standard vector size; one chunk holds at most this many rows.
const CHUNK_ROWS: usize = 2048;
use std::{
    error::Error,
    sync::atomic::{AtomicUsize, Ordering},
};

/// One output column: its name, its DuckDB type, and how to read it from a row.
struct Column {
    name: &'static str,
    kind: LogicalTypeId,
    path: &'static [&'static str],
}

/// Rows already fetched, ready to emit.
#[repr(C)]
struct FetchedRows {
    rows: Vec<Value>,
}

/// How far through the fetched rows we have got.
///
/// DuckDB asks for one chunk at a time and a chunk holds at most
/// `STANDARD_VECTOR_SIZE` rows, so a table function that writes every row into
/// the first chunk would overrun the vector. This cursor emits in chunks and
/// signals completion by returning an empty one.
#[repr(C)]
struct Cursor {
    offset: AtomicUsize,
}

/// Declare `columns` on the bind, fetch `endpoint`, and hand back the rows.
fn bind_and_fetch(
    bind: &BindInfo,
    columns: &[Column],
    endpoint: String,
    params: &[(&str, String)],
) -> Result<FetchedRows, Box<dyn Error>> {
    for column in columns {
        bind.add_result_column(column.name, LogicalTypeHandle::from(column.kind));
    }
    let payload = api::get(&endpoint, params)?;
    Ok(FetchedRows {
        rows: api::rows(&payload),
    })
}

/// Write the next chunk of fetched rows.
///
/// A missing or null field becomes SQL NULL rather than a zero or an empty
/// string: `val` is documented as `anyOf[number, null]`, and turning that into
/// 0.0 would invent a reading that was never published.
fn emit(
    output: &mut DataChunkHandle,
    columns: &[Column],
    rows: &[Value],
    cursor: &AtomicUsize,
) -> Result<(), Box<dyn Error>> {
    let start = cursor.load(Ordering::Relaxed);
    if start >= rows.len() {
        output.set_len(0);
        return Ok(());
    }
    let end = rows.len().min(start + CHUNK_ROWS);
    let batch = &rows[start..end];
    cursor.store(end, Ordering::Relaxed);

    for (index, column) in columns.iter().enumerate() {
        let mut vector = output.flat_vector(index);
        for (row_index, row) in batch.iter().enumerate() {
            match column.kind {
                LogicalTypeId::Double => match api::f64_at(row, column.path) {
                    // SAFETY: row_index < batch.len() <= CHUNK_ROWS, which is the
                    // vector's capacity, and the column was declared DOUBLE at bind.
                    Some(value) => unsafe { vector.as_mut_slice::<f64>()[row_index] = value },
                    None => vector.set_null(row_index),
                },
                LogicalTypeId::Bigint => match api::i64_at(row, column.path) {
                    // SAFETY: as above; the column was declared BIGINT at bind.
                    Some(value) => unsafe { vector.as_mut_slice::<i64>()[row_index] = value },
                    None => vector.set_null(row_index),
                },
                LogicalTypeId::Timestamp => {
                    // The API publishes epoch seconds; DuckDB timestamps are micros.
                    match api::i64_at(row, column.path).and_then(|s| s.checked_mul(1_000_000)) {
                        // SAFETY: as above; the column was declared TIMESTAMP at bind,
                        // which DuckDB stores as i64 microseconds.
                        Some(micros) => unsafe {
                            vector.as_mut_slice::<i64>()[row_index] = micros
                        },
                        None => vector.set_null(row_index),
                    }
                }
                _ => match api::str_at(row, column.path) {
                    Some(value) => vector.insert(row_index, value.as_str()),
                    None => vector.set_null(row_index),
                },
            }
        }
    }

    output.set_len(batch.len());
    Ok(())
}

/// Read one bind parameter as a lowercase string.
fn param(bind: &BindInfo, index: u64) -> String {
    bind.get_parameter(index).to_string().to_lowercase()
}

// ---------------------------------------------------------------------------
// fxmacrodata_announcements(currency, indicator)
// ---------------------------------------------------------------------------

const ANNOUNCEMENT_COLUMNS: &[Column] = &[
    Column { name: "date", kind: LogicalTypeId::Varchar, path: &["date"] },
    Column { name: "val", kind: LogicalTypeId::Double, path: &["val"] },
    Column {
        name: "announced_at",
        kind: LogicalTypeId::Timestamp,
        path: &["announcement_datetime"],
    },
    Column {
        name: "announced_at_local",
        kind: LogicalTypeId::Varchar,
        path: &["announcement_datetime_local"],
    },
    Column { name: "previous_value", kind: LogicalTypeId::Double, path: &["previous_value"] },
    Column { name: "source", kind: LogicalTypeId::Varchar, path: &["source"] },
    Column { name: "source_url", kind: LogicalTypeId::Varchar, path: &["source_url"] },
];

struct AnnouncementsVTab;

impl VTab for AnnouncementsVTab {
    type InitData = Cursor;
    type BindData = FetchedRows;

    fn bind(bind: &BindInfo) -> Result<Self::BindData, Box<dyn Error>> {
        let currency = param(bind, 0);
        let indicator = param(bind, 1);
        bind_and_fetch(
            bind,
            ANNOUNCEMENT_COLUMNS,
            format!("announcements/{currency}/{indicator}"),
            &[("limit", "100".to_string())],
        )
    }

    fn init(_: &InitInfo) -> Result<Self::InitData, Box<dyn Error>> {
        Ok(Cursor { offset: AtomicUsize::new(0) })
    }

    fn func(func: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> Result<(), Box<dyn Error>> {
        emit(output, ANNOUNCEMENT_COLUMNS, &func.get_bind_data().rows, &func.get_init_data().offset)
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![
            LogicalTypeHandle::from(LogicalTypeId::Varchar),
            LogicalTypeHandle::from(LogicalTypeId::Varchar),
        ])
    }
}

// ---------------------------------------------------------------------------
// fxmacrodata_calendar(currency)
// ---------------------------------------------------------------------------

const CALENDAR_COLUMNS: &[Column] = &[
    Column { name: "release", kind: LogicalTypeId::Varchar, path: &["release"] },
    Column { name: "name", kind: LogicalTypeId::Varchar, path: &["name"] },
    Column {
        name: "announced_at",
        kind: LogicalTypeId::Timestamp,
        path: &["announcement_datetime"],
    },
    Column {
        name: "announced_at_local",
        kind: LogicalTypeId::Varchar,
        path: &["announcement_datetime_local"],
    },
    Column {
        name: "event_importance",
        kind: LogicalTypeId::Varchar,
        path: &["event_importance"],
    },
    Column { name: "source", kind: LogicalTypeId::Varchar, path: &["source"] },
    Column { name: "source_url", kind: LogicalTypeId::Varchar, path: &["source_url"] },
];

struct CalendarVTab;

impl VTab for CalendarVTab {
    type InitData = Cursor;
    type BindData = FetchedRows;

    fn bind(bind: &BindInfo) -> Result<Self::BindData, Box<dyn Error>> {
        let currency = param(bind, 0);
        bind_and_fetch(
            bind,
            CALENDAR_COLUMNS,
            format!("calendar/{currency}"),
            &[("limit", "100".to_string())],
        )
    }

    fn init(_: &InitInfo) -> Result<Self::InitData, Box<dyn Error>> {
        Ok(Cursor { offset: AtomicUsize::new(0) })
    }

    fn func(func: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> Result<(), Box<dyn Error>> {
        emit(output, CALENDAR_COLUMNS, &func.get_bind_data().rows, &func.get_init_data().offset)
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }
}

// ---------------------------------------------------------------------------
// fxmacrodata_latest(currency)
// ---------------------------------------------------------------------------

const LATEST_COLUMNS: &[Column] = &[
    Column { name: "indicator", kind: LogicalTypeId::Varchar, path: &["indicator"] },
    Column { name: "name", kind: LogicalTypeId::Varchar, path: &["name"] },
    Column { name: "val", kind: LogicalTypeId::Double, path: &["latest", "val"] },
    Column { name: "unit", kind: LogicalTypeId::Varchar, path: &["unit"] },
    Column { name: "date", kind: LogicalTypeId::Varchar, path: &["latest", "date"] },
    Column {
        name: "announced_at",
        kind: LogicalTypeId::Timestamp,
        path: &["latest", "announcement_datetime"],
    },
    Column { name: "frequency", kind: LogicalTypeId::Varchar, path: &["frequency"] },
    Column { name: "source", kind: LogicalTypeId::Varchar, path: &["source"] },
];

struct LatestVTab;

impl VTab for LatestVTab {
    type InitData = Cursor;
    type BindData = FetchedRows;

    fn bind(bind: &BindInfo) -> Result<Self::BindData, Box<dyn Error>> {
        let currency = param(bind, 0);
        bind_and_fetch(bind, LATEST_COLUMNS, format!("announcements/{currency}/latest"), &[])
    }

    fn init(_: &InitInfo) -> Result<Self::InitData, Box<dyn Error>> {
        Ok(Cursor { offset: AtomicUsize::new(0) })
    }

    fn func(func: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> Result<(), Box<dyn Error>> {
        emit(output, LATEST_COLUMNS, &func.get_bind_data().rows, &func.get_init_data().offset)
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }
}

#[duckdb_entrypoint_c_api]
pub unsafe fn extension_entrypoint(con: Connection) -> Result<(), Box<dyn Error>> {
    con.register_table_function::<AnnouncementsVTab>("fxmacrodata_announcements")?;
    con.register_table_function::<CalendarVTab>("fxmacrodata_calendar")?;
    con.register_table_function::<LatestVTab>("fxmacrodata_latest")?;
    Ok(())
}
