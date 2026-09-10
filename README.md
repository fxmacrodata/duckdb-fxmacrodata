# duckdb-fxmacrodata

A DuckDB extension for querying [FXMacroData](https://fxmacrodata.com) — official-source
macroeconomic, FX and central-bank data across 18 currencies — as ordinary SQL tables.

```sql
LOAD fxmacrodata;

SELECT indicator, val, unit, announced_at, source
FROM fxmacrodata_latest('USD')
WHERE val IS NOT NULL
ORDER BY announced_at DESC;
```

```
gov_bond_10y              4.83   %   2026-09-09 20:15:00   US Treasury
breakeven_inflation_rate  2.37   %   2026-09-09 20:15:00   US Treasury
gov_bond_1m               3.81   %   2026-09-09 20:15:00   US Treasury
```

**No API key is needed for USD.**

## Why not just `read_json_auto`?

You can already query the API without this extension:

```sql
INSTALL httpfs; LOAD httpfs;
SELECT unnest(data) FROM read_json_auto('https://api.fxmacrodata.com/v1/announcements/usd/latest');
```

That works, and for a one-off it is fine. What the extension adds:

- **A name instead of a URL.** `fxmacrodata_latest('USD')` rather than an endpoint path.
- **Typed columns.** `val` is `DOUBLE` and `announced_at` is `TIMESTAMP`, so you can
  `ORDER BY`, compare and aggregate without casting. The raw JSON path gives you strings and
  a nested struct to `unnest`.
- **The key in a header.** Set `FXMACRODATA_API_KEY` and it travels as `X-API-Key`, so it is
  not written into a URL, a query plan, or a proxy log.
- **Nulls that stay null.** `val` is documented as `anyOf[number, null]`; a missing figure
  becomes SQL `NULL` rather than `0.0`.

## Functions

| Function | Returns |
| --- | --- |
| `fxmacrodata_latest(currency)` | The most recent print of **every** indicator for an economy, in one call |
| `fxmacrodata_announcements(currency, indicator)` | The published history of one indicator |
| `fxmacrodata_calendar(currency)` | Upcoming scheduled releases with publication times |

Every row carries the instant the figure was published, which is what makes the data usable
for point-in-time work rather than only for describing the present.

```sql
-- What is due next, and how important is it?
SELECT release, announced_at, event_importance
FROM fxmacrodata_calendar('USD')
WHERE announced_at > now()
ORDER BY announced_at
LIMIT 5;

-- How has US inflation moved, and when was each figure released?
SELECT date, val, announced_at, source
FROM fxmacrodata_announcements('USD', 'inflation')
ORDER BY date DESC;
```

## Authentication

USD data is public. For the other seventeen currencies and the full history window:

```bash
export FXMACRODATA_API_KEY=your-key   # FXMD_API_KEY is accepted as an alias
```

`FXMACRODATA_BASE_URL` can point the extension at a different deployment; the scheme is
pinned to `http`/`https`.

## Building

Requires Rust and the DuckDB extension toolchain:

```bash
make configure
make release
```

Or, to build and load the library directly:

```bash
cargo build --release
python extension-ci-tools/scripts/append_extension_metadata.py \
  -l target/release/fxmacrodata.dll -n fxmacrodata \
  -o fxmacrodata.duckdb_extension \
  -p "$(duckdb -c 'PRAGMA platform' -noheader -list)" \
  -dv v1.5.5 -ev 0.1.0 --abi-type C_STRUCT_UNSTABLE
```

Then, with `allow_unsigned_extensions` enabled:

```sql
LOAD '/absolute/path/to/fxmacrodata.duckdb_extension';
```

Run the Rust unit tests with `cargo test`, and the SQL tests with `make test`.

## License

MIT.
