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

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("data");

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

    let headers: Vec<String> = rdr
        .headers()?
        .iter()
        .map(|h| h.to_string())
        .collect();

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
    use calamine::{open_workbook_auto, Reader};

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
            Some(hdr) => hdr.iter().map(|c| cell_to_string(c)).collect(),
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
        Data::DateTime(d) => d.to_string(),
        Data::DateTimeIso(d) => d.clone(),
        Data::DurationIso(d) => d.clone(),
        Data::Error(e) => format!("#ERROR: {}", e),
    }
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
        let path =
            write_temp("csv", "Name,Note\nAlice,\"hello, world\"\nBob,\"say \"\"hi\"\"\"\n");
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
