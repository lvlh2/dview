use std::path::Path;

use anyhow::Result;
use calamine::Data;
use unicode_width::UnicodeWidthStr;

/// Unified data table representation loaded from any supported format.
#[derive(Debug)]
pub struct DataTable {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Max display width per column (capped at MAX_COL_WIDTH).
    pub column_widths: Vec<usize>,
}

impl DataTable {
    pub fn new(headers: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        let column_widths = Self::compute_widths(&headers, &rows);
        Self {
            headers,
            rows,
            column_widths,
        }
    }

    fn compute_widths(headers: &[String], rows: &[Vec<String>]) -> Vec<usize> {
        let n = headers.len();
        let mut widths: Vec<usize> = headers.iter().map(|h| h.width().max(4)).collect();
        for row in rows {
            for (i, cell) in row.iter().enumerate() {
                if i < n {
                    widths[i] = widths[i].max(cell.width().max(4));
                }
            }
        }
        widths
    }

    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn total_cols(&self) -> usize {
        self.headers.len()
    }
}

/// Detect format by extension and load the file.
/// Returns a list of (sheet_name, DataTable) pairs.
/// CSV/TSV/Parquet always return a single entry; Excel may return multiple.
pub fn load_file(path: &Path) -> Result<Vec<(String, DataTable)>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("data");

    match ext.as_str() {
        "csv" => Ok(vec![(stem.to_string(), load_csv(path, b',')?)]),
        "tsv" | "tab" => Ok(vec![(stem.to_string(), load_csv(path, b'\t')?)]),
        "xls" | "xlsx" | "xlsm" | "xlsb" => load_excel(path),
        "parquet" | "pq" => Ok(vec![(stem.to_string(), load_parquet(path)?)]),
        _ => Err(anyhow::anyhow!(
            "Unsupported file format: .{}\nSupported: csv, tsv, xls, xlsx, parquet",
            ext
        )),
    }
}

// ---------------------------------------------------------------------------
// CSV / TSV loader
// ---------------------------------------------------------------------------

fn load_csv(path: &Path, delimiter: u8) -> Result<DataTable> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(true)
        .from_path(path)?;

    let headers: Vec<String> = rdr.headers()?.iter().map(|h| h.to_string()).collect();

    let mut rows = Vec::new();
    for result in rdr.records() {
        let record = result?;
        let row: Vec<String> = (0..headers.len())
            .map(|i| record.get(i).unwrap_or("").to_string())
            .collect();
        rows.push(row);
    }

    Ok(DataTable::new(headers, rows))
}

// ---------------------------------------------------------------------------
// Excel loader (calamine)
// ---------------------------------------------------------------------------

fn load_excel(path: &Path) -> Result<Vec<(String, DataTable)>> {
    use calamine::{Reader, open_workbook_auto};

    let mut workbook = open_workbook_auto(path)?;
    let sheet_names = workbook.sheet_names().to_vec();

    if sheet_names.is_empty() {
        return Err(anyhow::anyhow!("Excel file has no sheets"));
    }

    let mut sheets = Vec::with_capacity(sheet_names.len());
    for name in &sheet_names {
        let range = workbook.worksheet_range(name)?;
        let mut rows_iter = range.rows();

        // First row = headers
        let headers: Vec<String> = match rows_iter.next() {
            Some(hdr) => hdr.iter().map(cell_to_string).collect(),
            None => {
                sheets.push((name.clone(), DataTable::new(vec![], vec![])));
                continue;
            }
        };

        let mut rows = Vec::new();
        for row in rows_iter {
            let cells: Vec<String> = (0..headers.len())
                .map(|i| {
                    if let Some(cell) = row.get(i) {
                        cell_to_string(cell)
                    } else {
                        String::new()
                    }
                })
                .collect();
            // Skip fully empty rows
            if cells.iter().any(|c| !c.is_empty()) {
                rows.push(cells);
            }
        }

        sheets.push((name.clone(), DataTable::new(headers, rows)));
    }

    Ok(sheets)
}

fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => {
            // Remove trailing zeros for cleaner display
            let s = format!("{:.10}", f);
            let s = s.trim_end_matches('0');
            s.trim_end_matches('.').to_string()
        }
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(d) => {
            let val = d.as_f64();
            if d.is_duration() {
                // Duration like [hh]:mm:ss — format as hours:minutes:seconds.
                let total_secs = (val * 86400.0).round() as i64;
                let h = total_secs / 3600;
                let m = (total_secs % 3600) / 60;
                let s = total_secs % 60;
                format!("{}:{:02}:{:02}", h, m, s)
            } else {
                excel_serial_to_date(val)
            }
        }
        Data::DateTimeIso(d) => d.clone(),
        Data::DurationIso(d) => d.clone(),
        Data::Error(e) => format!("#ERROR: {}", e),
    }
}

/// Convert an Excel serial date number to a YYYY-MM-DD string.
///
/// Excel date system (1900 date system):
///   - Serial 0   = Jan 0, 1900 (virtual) = 1899-12-31
///   - Serial 1   = Jan 1, 1900
///   - Serial 60  = Feb 29, 1900 (fictional — Excel treats 1900 as a leap year
///     for Lotus 1-2-3 compatibility)
///   - Serial 61+ = Mar 1, 1900 onward, but one day ahead of reality
///
/// The adjustment: for serial >= 61, subtract 1 to skip the nonexistent Feb 29.
/// Then add the adjusted number of days to 1899-12-31.
fn excel_serial_to_date(serial: f64) -> String {
    let mut days = serial.floor() as i64;
    if days <= 0 {
        return format!("{}", serial);
    }
    // Show the fictional Feb 29, 1900 as-is (Excel compatibility).
    if days == 60 {
        return "1900-02-29".to_string();
    }
    // After the fictional leap day, the count is off by one.
    if days > 60 {
        days -= 1;
    }
    days_to_ymd(days)
}

/// Convert days since 1899-12-31 to a YYYY-MM-DD string.
/// Simple day-by-day increment — fast enough for any practical Excel date.
fn days_to_ymd(days: i64) -> String {
    const MONTH_DAYS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    // Start at the Excel epoch: 1899-12-31 (day 0).
    let mut y: i64 = 1899;
    let mut m: i64 = 12;
    let mut d: i64 = 31;

    // Forward from epoch.
    if days >= 0 {
        for _ in 0..days {
            let mdays = if m == 2 {
                if is_leap(y) { 29 } else { 28 }
            } else {
                MONTH_DAYS[(m - 1) as usize]
            };
            d += 1;
            if d > mdays {
                d = 1;
                m += 1;
                if m > 12 {
                    m = 1;
                    y += 1;
                }
            }
        }
    } else {
        // Backward from epoch (negative days — shouldn't happen in practice).
        for _ in 0..(-days) {
            d -= 1;
            if d < 1 {
                m -= 1;
                if m < 1 {
                    m = 12;
                    y -= 1;
                }
                d = if m == 2 {
                    if is_leap(y) { 29 } else { 28 }
                } else {
                    MONTH_DAYS[(m - 1) as usize]
                };
            }
        }
    }

    format!("{}-{:02}-{:02}", y, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

// ---------------------------------------------------------------------------
// Parquet loader (parquet + arrow)
// ---------------------------------------------------------------------------

fn load_parquet(path: &Path) -> Result<DataTable> {
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use std::fs::File;

    let file = File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let reader = builder.build()?;

    let mut headers = Vec::new();
    let mut rows = Vec::new();

    for batch_result in reader {
        let batch = batch_result?;
        let schema = batch.schema();

        if headers.is_empty() {
            for field in schema.fields() {
                headers.push(field.name().clone());
            }
        }

        for row_idx in 0..batch.num_rows() {
            let mut row = Vec::with_capacity(batch.num_columns());
            for col_idx in 0..batch.num_columns() {
                let column = batch.column(col_idx);
                row.push(arrow_value_to_str(column, row_idx));
            }
            rows.push(row);
        }
    }

    if headers.is_empty() {
        return Ok(DataTable::new(vec![], vec![]));
    }

    Ok(DataTable::new(headers, rows))
}

/// Convert a single arrow array value at `idx` to a string.
fn arrow_value_to_str(arr: &dyn arrow::array::Array, idx: usize) -> String {
    if arr.is_null(idx) {
        return String::new();
    }

    use arrow::array::*;

    macro_rules! downcast {
        ($ty:ty, $arr:expr, $idx:expr) => {
            if let Some(a) = $arr.as_any().downcast_ref::<$ty>() {
                return a.value($idx).to_string();
            }
        };
    }

    downcast!(Int8Array, arr, idx);
    downcast!(Int16Array, arr, idx);
    downcast!(Int32Array, arr, idx);
    downcast!(Int64Array, arr, idx);
    downcast!(UInt8Array, arr, idx);
    downcast!(UInt16Array, arr, idx);
    downcast!(UInt32Array, arr, idx);
    downcast!(UInt64Array, arr, idx);
    downcast!(Float16Array, arr, idx);
    downcast!(Float32Array, arr, idx);
    downcast!(Float64Array, arr, idx);
    downcast!(StringArray, arr, idx);
    downcast!(LargeStringArray, arr, idx);
    downcast!(BooleanArray, arr, idx);

    // Fallback: try to get a reasonable string representation.
    String::new()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(ext: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("dview_test_{}.{}", uuid_simple(), ext));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    fn uuid_simple() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        format!("{:08x}", t)
    }

    // -----------------------------------------------------------------------
    // CSV basic
    // -----------------------------------------------------------------------

    #[test]
    fn test_load_csv() {
        let path = write_temp("csv", "Name,Age,Score\nAlice,30,95.5\nBob,25,87.3\n");
        let sheets = load_file(&path).unwrap();
        let table = &sheets[0].1;
        assert_eq!(table.headers, vec!["Name", "Age", "Score"]);
        assert_eq!(table.total_rows(), 2);
        assert_eq!(table.total_cols(), 3);
        assert_eq!(table.rows[0], vec!["Alice", "30", "95.5"]);
        assert_eq!(table.rows[1], vec!["Bob", "25", "87.3"]);
        for w in &table.column_widths {
            assert!(*w >= 4);
        }
        std::fs::remove_file(&path).ok();
    }

    // -----------------------------------------------------------------------
    // TSV
    // -----------------------------------------------------------------------

    #[test]
    fn test_load_tsv() {
        let path = write_temp("tsv", "ColA\tColB\nx1\ty1\n");
        let sheets = load_file(&path).unwrap();
        let table = &sheets[0].1;
        assert_eq!(table.headers, vec!["ColA", "ColB"]);
        assert_eq!(table.total_rows(), 1);
        assert_eq!(table.rows[0], vec!["x1", "y1"]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_load_tab_extension() {
        let path = write_temp("tab", "X\tY\n1\t2\n");
        let sheets = load_file(&path).unwrap();
        let table = &sheets[0].1;
        assert_eq!(table.headers, vec!["X", "Y"]);
        assert_eq!(table.total_rows(), 1);
        std::fs::remove_file(&path).ok();
    }

    // -----------------------------------------------------------------------
    // CSV edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_csv_empty_fields() {
        let path = write_temp("csv", "A,B,C\n,,\nalpha,,gamma\n");
        let sheets = load_file(&path).unwrap();
        let table = &sheets[0].1;
        assert_eq!(table.rows[0], vec!["", "", ""]);
        assert_eq!(table.rows[1], vec!["alpha", "", "gamma"]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_csv_quoted_fields() {
        let path = write_temp(
            "csv",
            "Name,Note\nAlice,\"hello, world\"\nBob,\"say \"\"hi\"\"\"\n",
        );
        let sheets = load_file(&path).unwrap();
        let table = &sheets[0].1;
        assert_eq!(table.rows[0][1], "hello, world");
        assert!(table.rows[1][1].contains("hi"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_csv_headers_only() {
        let path = write_temp("csv", "Col1,Col2,Col3\n");
        let sheets = load_file(&path).unwrap();
        let table = &sheets[0].1;
        assert_eq!(table.headers.len(), 3);
        assert_eq!(table.total_rows(), 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_csv_single_column() {
        let path = write_temp("csv", "Only\n1\n2\n3\n");
        let sheets = load_file(&path).unwrap();
        let table = &sheets[0].1;
        assert_eq!(table.total_cols(), 1);
        assert_eq!(table.total_rows(), 3);
        assert_eq!(table.rows[2], vec!["3"]);
        std::fs::remove_file(&path).ok();
    }

    // -----------------------------------------------------------------------
    // CJK / Unicode width
    // -----------------------------------------------------------------------

    #[test]
    fn test_cjk_column_widths() {
        let path = write_temp("csv", "ID,城市\n1,北京\n2,上海\n");
        let sheets = load_file(&path).unwrap();
        let table = &sheets[0].1;
        // "城市" / "北京" / "上海" each has width 4 (2 CJK chars × 2).
        // Column widths should be ≥ 4 with .max(4).
        assert_eq!(table.column_widths[0], 4); // "ID" → 2, max(4) → 4
        assert_eq!(table.column_widths[1], 4); // "城市"/"北京"/"上海" → 4
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_cjk_data_loaded_correctly() {
        let path = write_temp("csv", "名称,价格\n苹果,5\n香蕉,3\n");
        let sheets = load_file(&path).unwrap();
        let table = &sheets[0].1;
        assert_eq!(table.rows[0], vec!["苹果", "5"]);
        assert_eq!(table.rows[1], vec!["香蕉", "3"]);
        std::fs::remove_file(&path).ok();
    }

    // -----------------------------------------------------------------------
    // Unsupported format
    // -----------------------------------------------------------------------

    #[test]
    fn test_excel_serial_date_conversion() {
        // 1900 date system (no leap year bug adjustment needed for serials <= 60).
        assert_eq!(excel_serial_to_date(1.0), "1900-01-01");
        assert_eq!(excel_serial_to_date(59.0), "1900-02-28");
        assert_eq!(excel_serial_to_date(60.0), "1900-02-29"); // fictional leap day
        assert_eq!(excel_serial_to_date(61.0), "1900-03-01");
        // Anchor: Jan 1, 2000 = serial 36526.
        assert_eq!(excel_serial_to_date(36526.0), "2000-01-01");
        // Serial 42380 = Jan 11, 2016.
        assert_eq!(excel_serial_to_date(42380.0), "2016-01-11");
        // Date with time component (fractional part ignored).
        assert_eq!(excel_serial_to_date(42380.5), "2016-01-11");
        // Edge: zero or negative returns raw.
        assert_eq!(excel_serial_to_date(0.0), "0");
    }

    #[test]
    fn test_unsupported_extension() {
        let path = write_temp("json", "{}");
        let err = load_file(&path).unwrap_err();
        assert!(err.to_string().contains("Unsupported"));
        std::fs::remove_file(&path).ok();
    }

    // -----------------------------------------------------------------------
    // DataTable construction
    // -----------------------------------------------------------------------

    #[test]
    fn test_datatable_empty() {
        let table = DataTable::new(vec![], vec![]);
        assert_eq!(table.total_rows(), 0);
        assert_eq!(table.total_cols(), 0);
        assert!(table.column_widths.is_empty());
    }

    fn dt(f: f64) -> calamine::Data {
        calamine::Data::DateTime(calamine::ExcelDateTime::new(
            f,
            calamine::ExcelDateTimeType::DateTime,
            false,
        ))
    }
    fn dt_iso(s: &str) -> calamine::Data {
        calamine::Data::DateTimeIso(s.into())
    }

    #[test]
    fn test_cell_to_string_datetime() {
        // DateTime variant should produce a readable date string.
        assert_eq!(cell_to_string(&dt(42380.0)), "2016-01-11");
        assert_eq!(cell_to_string(&dt(36526.0)), "2000-01-01");
        // DateTimeIso passes through unchanged.
        assert_eq!(
            cell_to_string(&dt_iso("2024-03-15T10:30:00")),
            "2024-03-15T10:30:00"
        );
    }

    #[test]
    fn test_datatable_compute_widths() {
        let table = DataTable::new(
            vec!["short".into(), "very_long_header".into()],
            vec![vec!["x".into(), "data".into()]],
        );
        assert_eq!(table.column_widths[0], 5); // "short" → 5
        assert_eq!(table.column_widths[1], 16); // "very_long_header" → 16
    }
}
