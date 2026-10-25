//! The dataset manifest: what exists, per dataset version, for
//! `GET /datasets` to serve and sign. Built by `0xbt-build-api manifest`.
//!
//! Coverage is per column: `column_starts` gives the first UTC date each column has a
//! value, so a user sees before running that, say, liquidations are null before
//! 2025-12-22. A column with no value anywhere is null there.

use std::collections::BTreeMap;

use arrow_array::{Array, Int64Array};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::{Deserialize, Serialize};

use crate::ParquetError;

pub const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub manifest_version: u32,
    pub schema_version: String,
    pub dataset_version: String,
    /// Sorted by market name.
    pub markets: Vec<MarketEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarketEntry {
    /// `hyperliquid:BTC` form.
    pub market: String,
    pub rows: u64,
    /// One per month, oldest first.
    pub files: Vec<FileEntry>,
    /// Column name to the first UTC date (`YYYY-MM-DD`) with a value, or null.
    pub column_starts: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    /// `YYYY-MM`.
    pub month: String,
    /// Relative to the manifest, `/`-separated.
    pub path: String,
    pub bytes: u64,
    pub rows: u64,
    /// SHA-256 of the file, hex.
    pub sha256: String,
}

/// The `ts` of the first row with a value, per column, in one file.
pub fn first_values(bytes: bytes::Bytes) -> Result<BTreeMap<String, Option<i64>>, ParquetError> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(bytes)?.build()?;
    let mut out: BTreeMap<String, Option<i64>> = BTreeMap::new();
    for batch in reader {
        let batch = batch?;
        let schema = batch.schema();
        let ts = batch
            .column_by_name("ts")
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .ok_or_else(|| ParquetError::General("no ts column".into()))?;
        for (i, field) in schema.fields().iter().enumerate() {
            if field.name() == "ts" {
                continue;
            }
            let slot = out.entry(field.name().clone()).or_insert(None);
            if slot.is_none() {
                let col = batch.column(i);
                *slot = (0..col.len())
                    .find(|&r| col.is_valid(r))
                    .map(|r| ts.value(r));
            }
        }
    }
    Ok(out)
}

/// `YYYY-MM-DD` for a Unix-ms time, UTC (Howard Hinnant's civil_from_days).
pub fn utc_date(ms: i64) -> String {
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_791_208_800_000), "2026-10-05");
        assert_eq!(utc_date(951_782_400_000), "2000-02-29");
        assert_eq!(utc_date(-1), "1969-12-31");
    }
}
