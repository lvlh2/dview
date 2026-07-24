use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::Result;
use calamine::Data;
use unicode_width::UnicodeWidthStr;

// ---------------------------------------------------------------------------
// DataTable — owns headers, column widths, and a row backend
// ---------------------------------------------------------------------------

/// Unified data table representation. Headers and column widths are always
/// in memory; rows are provided by a `RowBackend` that may be lazy.
#[derive(Debug)]
pub struct DataTable {
    pub headers: Vec<String>,
    /// Max display width per column (min 4), updated as rows are read.
    pub column_widths: Vec<usize>,
    backend: RowBackend,
}

impl DataTable {
    /// Build an in-memory table (used by Excel loaders and tests).
    pub fn new(headers: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        let column_widths = Self::compute_widths(&headers, &rows);
        Self {
            headers,
            column_widths,
            backend: RowBackend::InMemory(rows),
        }
    }

    pub fn total_rows(&self) -> usize {
        self.backend.total_rows()
    }

    pub fn total_cols(&self) -> usize {
        self.headers.len()
    }

    /// Get a single row by index. For lazy backends this may read from
    /// disk and populate the cache; call `prefetch_range` before rendering
    /// to warm the cache for the visible viewport.
    pub fn get_row(&mut self, idx: usize) -> &[String] {
        self.backend.get_row(idx)
    }

    /// Ensure rows `[start, start+count)` are in cache.
    pub fn prefetch_range(&mut self, start: usize, count: usize) {
        self.backend.prefetch_range(start, count, &self.headers);
    }

    /// Update column widths from a single row (opportunistic refinement
    /// for lazy backends whose initial widths were sample-based).
    #[allow(dead_code)]
    pub fn refine_widths(&mut self, row: &[String]) {
        for (i, cell) in row.iter().enumerate() {
            if i < self.column_widths.len() {
                let w = cell.width().max(4);
                if w > self.column_widths[i] {
                    self.column_widths[i] = w;
                }
            }
        }
    }

    /// Full-scan width computation (used for InMemory backend).
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
}

// ---------------------------------------------------------------------------
// RowBackend — the lazy/eager row storage
// ---------------------------------------------------------------------------

enum RowBackend {
    InMemory(Vec<Vec<String>>),
    Csv(CsvBackend),
    Parquet(ParquetBackend),
}

impl std::fmt::Debug for RowBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InMemory(rows) => f.debug_tuple("InMemory").field(&rows.len()).finish(),
            Self::Csv(_) => f.debug_tuple("Csv").finish(),
            Self::Parquet(_) => f.debug_tuple("Parquet").finish(),
        }
    }
}

impl RowBackend {
    fn total_rows(&self) -> usize {
        match self {
            RowBackend::InMemory(rows) => rows.len(),
            RowBackend::Csv(b) => b.total_rows,
            RowBackend::Parquet(b) => b.total_rows,
        }
    }

    fn get_row(&mut self, idx: usize) -> &[String] {
        match self {
            RowBackend::InMemory(rows) => &rows[idx],
            RowBackend::Csv(b) => b.get_row(idx),
            RowBackend::Parquet(b) => b.get_row(idx),
        }
    }

    fn prefetch_range(&mut self, start: usize, count: usize, headers: &[String]) {
        match self {
            RowBackend::InMemory(_) => { /* all rows already in memory */ }
            RowBackend::Csv(b) => b.prefetch_range(start, count, headers.len()),
            RowBackend::Parquet(b) => b.prefetch_range(start, count, headers.len()),
        }
    }
}

// ---------------------------------------------------------------------------
// CsvBackend — byte-offset indexed, lazy loading
// ---------------------------------------------------------------------------

/// Number of rows to load in one I/O operation when the cache misses.
const CSV_CACHE_WINDOW: usize = 500;

struct CsvBackend {
    /// Buffered file reader positioned arbitrarily by seeks.
    reader: BufReader<File>,
    /// Byte offset of the start of each data row.  `offsets[i]` is the first
    /// byte of row `i`.  There are `total_rows + 1` entries so that
    /// `offsets[i+1]` gives the exclusive end of row `i`.
    row_offsets: Vec<u64>,
    delimiter: u8,
    total_rows: usize,
    /// Index of the first row currently held in `cache`.
    cache_start: usize,
    cache: Vec<Vec<String>>,
}

impl CsvBackend {
    /// Build a byte-offset index from a CSV/TSV file.  Returns headers,
    /// estimated column widths, and the backend.
    fn open(path: &Path, delimiter: u8) -> Result<(Vec<String>, Vec<usize>, Self)> {
        let file = File::open(path)?;
        let file_size = file.metadata()?.len();

        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(delimiter)
            .has_headers(true)
            .from_reader(file);

        // Read header row.
        let headers: Vec<String> = rdr.headers()?.iter().map(|h| h.to_string()).collect();
        let num_cols = headers.len();
        let mut widths: Vec<usize> = headers.iter().map(|h| h.width().max(4)).collect();

        // Record the byte position at the start of the first data row.
        let first_data_offset = rdr.position().byte();
        let mut offsets: Vec<u64> = Vec::with_capacity(1024);
        offsets.push(first_data_offset);

        // Sample rate for width tracking on large files: check every K-th row.
        let estimated_rows = file_size
            .checked_div(20) // rough guess: average row ~20 bytes
            .unwrap_or(0);
        let sample_interval = if estimated_rows > 100_000 {
            (estimated_rows / 2000).max(1) as usize
        } else {
            1 // exact widths for small files
        };

        let mut row_count: usize = 0;
        let mut record = csv::ByteRecord::new();
        while rdr.read_byte_record(&mut record)? {
            offsets.push(rdr.position().byte());

            // Track widths from sampled rows only.
            if sample_interval == 1 || row_count % sample_interval == 0 {
                for (i, field) in record.iter().enumerate() {
                    if i < num_cols {
                        let s = std::str::from_utf8(field).unwrap_or("");
                        widths[i] = widths[i].max(s.width().max(4));
                    }
                }
            }
            row_count += 1;
        }

        let total_rows = offsets.len().saturating_sub(1); // last offset = EOF
        let reader = BufReader::new(rdr.into_inner());

        Ok((
            headers,
            widths,
            Self {
                reader,
                row_offsets: offsets,
                delimiter,
                total_rows,
                cache_start: 0,
                cache: Vec::new(),
            },
        ))
    }

    fn get_row(&mut self, idx: usize) -> &[String] {
        // Fast path: row already in cache.
        if idx >= self.cache_start && idx < self.cache_start + self.cache.len() {
            return &self.cache[idx - self.cache_start];
        }
        // Load a window around the requested row.
        let window_start = idx.saturating_sub(CSV_CACHE_WINDOW / 2);
        let window_end = (window_start + CSV_CACHE_WINDOW).min(self.total_rows);
        self.load_range(window_start, window_end);
        &self.cache[idx - self.cache_start]
    }

    fn prefetch_range(&mut self, start: usize, count: usize, _num_cols: usize) {
        let end = (start + count).min(self.total_rows);
        if end == 0 {
            return;
        }
        // Already cached?
        if start >= self.cache_start && end <= self.cache_start + self.cache.len() {
            return;
        }
        // Load a window that covers the requested range with some margin.
        let margin = 100usize;
        let window_start = start.saturating_sub(margin);
        let window_end = (end + margin).min(self.total_rows).max(window_start + 1);
        self.load_range(window_start, window_end);
    }

    fn load_range(&mut self, start: usize, end: usize) {
        if start >= end || end > self.total_rows {
            return;
        }

        let byte_start = self.row_offsets[start];
        let byte_end = self.row_offsets[end]; // sentinel ensures this exists
        let chunk_size = (byte_end - byte_start) as usize;

        let mut buf = vec![0u8; chunk_size];
        if self.reader.seek(SeekFrom::Start(byte_start)).is_err() {
            // On seek failure, clear cache — caller will get empty row.
            self.cache.clear();
            self.cache_start = start;
            return;
        }
        if self.reader.read_exact(&mut buf).is_err() {
            self.cache.clear();
            self.cache_start = start;
            return;
        }

        // Parse the chunk into rows.
        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(self.delimiter)
            .has_headers(false)
            .from_reader(&buf[..]);

        let mut rows: Vec<Vec<String>> = Vec::with_capacity(end - start);
        for result in rdr.records() {
            if let Ok(record) = result {
                let row: Vec<String> = (0..record.len())
                    .map(|i| record.get(i).unwrap_or("").to_string())
                    .collect();
                rows.push(row);
            }
        }

        self.cache_start = start;
        self.cache = rows;
    }
}

// ---------------------------------------------------------------------------
// ParquetBackend — row-group-level lazy loading with targeted row reads
// ---------------------------------------------------------------------------

/// Number of extra rows to include around the visible window on each side.
const PARQUET_CACHE_MARGIN: usize = 200;

struct ParquetBackend {
    /// File path for on-demand reads.
    path: std::path::PathBuf,
    /// (start_row, num_rows) for each row group in the file.
    row_group_bounds: Vec<(usize, usize)>,
    total_rows: usize,
    /// Which row group index is currently cached (if any).
    cached_group_idx: Option<usize>,
    /// Absolute row index of the first entry in `cache`.
    cache_start: usize,
    cache: Vec<Vec<String>>,
}

impl ParquetBackend {
    fn open(path: &Path) -> Result<(Vec<String>, Vec<usize>, Self)> {
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
        use std::fs::File;

        let file = File::open(path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
        let parquet_meta = builder.metadata().clone();
        let schema = builder.schema();

        // Extract headers from the schema.
        let headers: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
        let num_cols = headers.len();

        // Collect row group boundaries.
        let mut row_group_bounds: Vec<(usize, usize)> = Vec::new();
        let mut cumulative: usize = 0;
        for rg_idx in 0..parquet_meta.num_row_groups() {
            let rg = parquet_meta.row_group(rg_idx);
            let n = rg.num_rows() as usize;
            row_group_bounds.push((cumulative, n));
            cumulative += n;
        }
        let total_rows = cumulative;

        // Estimate column widths from the first row group.
        let mut widths: Vec<usize> = headers.iter().map(|h| h.width().max(4)).collect();
        if total_rows > 0 {
            // Read first few rows to estimate widths.
            let sample_count = 2000usize.min(total_rows);
            let (_, rows) = Self::read_row_window(
                path,
                &row_group_bounds,
                0,
                0, // offset within row group
                sample_count,
            )?;
            for row in &rows {
                for (i, cell) in row.iter().enumerate() {
                    if i < num_cols {
                        widths[i] = widths[i].max(cell.width().max(4));
                    }
                }
            }
            // Cache the sampled rows (covers initial viewport).
            Ok((
                headers,
                widths,
                Self {
                    path: path.to_path_buf(),
                    row_group_bounds,
                    total_rows,
                    cached_group_idx: Some(0),
                    cache_start: 0,
                    cache: rows,
                },
            ))
        } else {
            Ok((
                headers,
                widths,
                Self {
                    path: path.to_path_buf(),
                    row_group_bounds,
                    total_rows,
                    cached_group_idx: None,
                    cache_start: 0,
                    cache: Vec::new(),
                },
            ))
        }
    }

    fn get_row(&mut self, idx: usize) -> &[String] {
        // Fast path: in cache.
        if idx >= self.cache_start && idx < self.cache_start + self.cache.len() {
            return &self.cache[idx - self.cache_start];
        }

        // Find which row group contains this row.
        let rg_idx = find_row_group(&self.row_group_bounds, idx);

        // Load a window around the target row from that row group.
        let group_start = self.row_group_bounds[rg_idx].0;
        let group_rows = self.row_group_bounds[rg_idx].1;
        let offset = idx - group_start;

        // Read a small window around the requested row.
        let window_off = offset.saturating_sub(PARQUET_CACHE_MARGIN);
        let window_count = (offset + PARQUET_CACHE_MARGIN + 1)
            .min(group_rows)
            .saturating_sub(window_off);

        if let Ok((start, rows)) = Self::read_row_window(
            &self.path,
            &self.row_group_bounds,
            rg_idx,
            window_off,
            window_count,
        ) {
            self.cache_start = start;
            self.cache = rows;
            self.cached_group_idx = Some(rg_idx);
        } else {
            self.cache_start = idx;
            self.cache = vec![vec![]];
            self.cached_group_idx = Some(rg_idx);
        }

        // Return the requested row.
        if idx >= self.cache_start && idx < self.cache_start + self.cache.len() {
            &self.cache[idx - self.cache_start]
        } else {
            &self.cache[0]
        }
    }

    fn prefetch_range(&mut self, start: usize, count: usize, _num_cols: usize) {
        let end = (start + count).min(self.total_rows);
        if end == 0 || start >= end {
            return;
        }

        // Already fully in cache?
        if start >= self.cache_start && end <= self.cache_start + self.cache.len() {
            return;
        }

        let rg_idx = find_row_group(&self.row_group_bounds, start);
        let group_start = self.row_group_bounds[rg_idx].0;
        let group_rows = self.row_group_bounds[rg_idx].1;

        // Compute the window to read: start..end + margin on each side.
        let window_off = start
            .saturating_sub(group_start)
            .saturating_sub(PARQUET_CACHE_MARGIN);
        let window_end = (end - group_start + PARQUET_CACHE_MARGIN).min(group_rows);
        let window_count = window_end.saturating_sub(window_off);

        if window_count == 0 {
            return;
        }

        if let Ok((abs_start, rows)) = Self::read_row_window(
            &self.path,
            &self.row_group_bounds,
            rg_idx,
            window_off,
            window_count,
        ) {
            self.cache_start = abs_start;
            self.cache = rows;
            self.cached_group_idx = Some(rg_idx);
        }
    }

    /// Read a specific range of rows within a row group. Uses
    /// `with_row_selection` to only read/decompress the needed rows
    /// from disk, avoiding full row group decompression.
    fn read_row_window(
        path: &Path,
        bounds: &[(usize, usize)],
        rg_idx: usize,
        offset_within_group: usize,
        count: usize,
    ) -> Result<(usize, Vec<Vec<String>>)> {
        use parquet::arrow::arrow_reader::{
            ParquetRecordBatchReaderBuilder, RowSelection, RowSelector,
        };
        use std::fs::File;

        let group_start = bounds[rg_idx].0;
        let group_rows = bounds[rg_idx].1;
        let offset = offset_within_group.min(group_rows.saturating_sub(1));
        let limit = count.min(group_rows - offset);

        if limit == 0 {
            return Ok((group_start + offset, Vec::new()));
        }

        let file = File::open(path)?;
        let selection =
            RowSelection::from(vec![RowSelector::skip(offset), RowSelector::select(limit)]);
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)?
            .with_row_groups(vec![rg_idx])
            .with_row_selection(selection);
        let reader = builder.build()?;

        let mut rows: Vec<Vec<String>> = Vec::with_capacity(limit);
        for batch_result in reader {
            let batch = batch_result?;
            for row_idx in 0..batch.num_rows() {
                let mut row: Vec<String> = Vec::with_capacity(batch.num_columns());
                for col_idx in 0..batch.num_columns() {
                    let column = batch.column(col_idx);
                    row.push(arrow_value_to_str(column, row_idx));
                }
                rows.push(row);
            }
        }

        Ok((group_start + offset, rows))
    }
}

/// Binary search to find which row group contains `row_idx`.
fn find_row_group(bounds: &[(usize, usize)], row_idx: usize) -> usize {
    bounds
        .binary_search_by(|&(start, num)| {
            if row_idx < start {
                std::cmp::Ordering::Greater
            } else if row_idx >= start + num {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Arrow value → string helper (used by Parquet reader)
// ---------------------------------------------------------------------------

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

    String::new()
}

// ---------------------------------------------------------------------------
// Format detection / file loading
// ---------------------------------------------------------------------------

/// Detect format by extension and load the file.
/// Returns a list of (sheet_name, DataTable) pairs.
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
    let (headers, column_widths, backend) = CsvBackend::open(path, delimiter)?;
    Ok(DataTable {
        headers,
        column_widths,
        backend: RowBackend::Csv(backend),
    })
}

// ---------------------------------------------------------------------------
// Excel loader (calamine) — unchanged, always InMemory
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
            let s = format!("{:.10}", f);
            let s = s.trim_end_matches('0');
            s.trim_end_matches('.').to_string()
        }
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(d) => {
            let val = d.as_f64();
            if d.is_duration() {
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

fn excel_serial_to_date(serial: f64) -> String {
    let mut days = serial.floor() as i64;
    if days <= 0 {
        return format!("{}", serial);
    }
    if days == 60 {
        return "1900-02-29".to_string();
    }
    if days > 60 {
        days -= 1;
    }
    days_to_ymd(days)
}

fn days_to_ymd(days: i64) -> String {
    const MONTH_DAYS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut y: i64 = 1899;
    let mut m: i64 = 12;
    let mut d: i64 = 31;

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
// Parquet loader
// ---------------------------------------------------------------------------

fn load_parquet(path: &Path) -> Result<DataTable> {
    let (headers, column_widths, backend) = ParquetBackend::open(path)?;
    Ok(DataTable {
        headers,
        column_widths,
        backend: RowBackend::Parquet(backend),
    })
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

        // CSV uses lazy backend — check via get_row.
        let mut table = load_file(&path).unwrap().into_iter().next().unwrap().1;
        assert_eq!(table.get_row(0), vec!["Alice", "30", "95.5"]);
        assert_eq!(table.get_row(1), vec!["Bob", "25", "87.3"]);
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
        let mut table = load_file(&path).unwrap().into_iter().next().unwrap().1;
        assert_eq!(table.headers, vec!["ColA", "ColB"]);
        assert_eq!(table.total_rows(), 1);
        assert_eq!(table.get_row(0), vec!["x1", "y1"]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_load_tab_extension() {
        let path = write_temp("tab", "X\tY\n1\t2\n");
        let table = load_file(&path).unwrap().into_iter().next().unwrap().1;
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
        let mut table = load_file(&path).unwrap().into_iter().next().unwrap().1;
        assert_eq!(table.get_row(0), vec!["", "", ""]);
        assert_eq!(table.get_row(1), vec!["alpha", "", "gamma"]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_csv_quoted_fields() {
        let path = write_temp(
            "csv",
            "Name,Note\nAlice,\"hello, world\"\nBob,\"say \"\"hi\"\"\"\n",
        );
        let mut table = load_file(&path).unwrap().into_iter().next().unwrap().1;
        assert_eq!(table.get_row(0)[1], "hello, world");
        assert!(table.get_row(1)[1].contains("hi"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_csv_headers_only() {
        let path = write_temp("csv", "Col1,Col2,Col3\n");
        let table = load_file(&path).unwrap().into_iter().next().unwrap().1;
        assert_eq!(table.headers.len(), 3);
        assert_eq!(table.total_rows(), 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_csv_single_column() {
        let path = write_temp("csv", "Only\n1\n2\n3\n");
        let mut table = load_file(&path).unwrap().into_iter().next().unwrap().1;
        assert_eq!(table.total_cols(), 1);
        assert_eq!(table.total_rows(), 3);
        assert_eq!(table.get_row(2), vec!["3"]);
        std::fs::remove_file(&path).ok();
    }

    // -----------------------------------------------------------------------
    // CJK / Unicode width
    // -----------------------------------------------------------------------

    #[test]
    fn test_cjk_column_widths() {
        let path = write_temp("csv", "ID,城市\n1,北京\n2,上海\n");
        let table = load_file(&path).unwrap().into_iter().next().unwrap().1;
        assert_eq!(table.column_widths[0], 4);
        assert_eq!(table.column_widths[1], 4);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_cjk_data_loaded_correctly() {
        let path = write_temp("csv", "名称,价格\n苹果,5\n香蕉,3\n");
        let mut table = load_file(&path).unwrap().into_iter().next().unwrap().1;
        assert_eq!(table.get_row(0), vec!["苹果", "5"]);
        assert_eq!(table.get_row(1), vec!["香蕉", "3"]);
        std::fs::remove_file(&path).ok();
    }

    // -----------------------------------------------------------------------
    // Excel date conversion
    // -----------------------------------------------------------------------

    #[test]
    fn test_excel_serial_date_conversion() {
        assert_eq!(excel_serial_to_date(1.0), "1900-01-01");
        assert_eq!(excel_serial_to_date(59.0), "1900-02-28");
        assert_eq!(excel_serial_to_date(60.0), "1900-02-29");
        assert_eq!(excel_serial_to_date(61.0), "1900-03-01");
        assert_eq!(excel_serial_to_date(36526.0), "2000-01-01");
        assert_eq!(excel_serial_to_date(42380.0), "2016-01-11");
        assert_eq!(excel_serial_to_date(42380.5), "2016-01-11");
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
    // DataTable construction (InMemory)
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
        assert_eq!(cell_to_string(&dt(42380.0)), "2016-01-11");
        assert_eq!(cell_to_string(&dt(36526.0)), "2000-01-01");
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
        assert_eq!(table.column_widths[0], 5);
        assert_eq!(table.column_widths[1], 16);
    }

    // -----------------------------------------------------------------------
    // InMemory backend — get_row
    // -----------------------------------------------------------------------

    #[test]
    fn test_datatable_get_row_inmemory() {
        let mut table = DataTable::new(
            vec!["A".into(), "B".into()],
            vec![vec!["1".into(), "2".into()], vec!["3".into(), "4".into()]],
        );
        assert_eq!(table.get_row(0), &["1", "2"]);
        assert_eq!(table.get_row(1), &["3", "4"]);
    }

    #[test]
    fn test_refine_widths() {
        let mut table = DataTable::new(vec!["X".into()], vec![vec!["short".into()]]);
        let initial = table.column_widths[0];
        table.refine_widths(&["much_longer_value".into()]);
        assert!(table.column_widths[0] > initial);
        assert_eq!(table.column_widths[0], 17); // "much_longer_value".width()
    }

    // -----------------------------------------------------------------------
    // CsvBackend specific tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_csv_backend_cache_reload() {
        // Create a CSV with enough rows to exceed the cache window.
        let mut csv = String::from("A,B\n");
        for i in 0..1200 {
            csv.push_str(&format!("r{},c{}\n", i, i));
        }
        let path = write_temp("csv", &csv);
        let mut table = load_file(&path).unwrap().into_iter().next().unwrap().1;

        // Read row 0 — should be in cache.
        assert_eq!(table.get_row(0), &["r0", "c0"]);
        // Read row 1000 — far away, triggers cache reload.
        assert_eq!(table.get_row(1000), &["r1000", "c1000"]);
        // Row 0 may no longer be cached, but re-reading should work anyway.
        assert_eq!(table.get_row(0), &["r0", "c0"]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_csv_backend_empty() {
        let path = write_temp("csv", "H1,H2\n");
        let table = load_file(&path).unwrap().into_iter().next().unwrap().1;
        assert_eq!(table.total_rows(), 0);
        assert_eq!(table.headers.len(), 2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_csv_backend_quoted_newlines() {
        let path = write_temp("csv", "Col\n\"line1\nline2\"\n\"normal\"\n");
        let mut table = load_file(&path).unwrap().into_iter().next().unwrap().1;
        assert_eq!(table.total_rows(), 2);
        assert_eq!(table.get_row(0), &["line1\nline2"]);
        assert_eq!(table.get_row(1), &["normal"]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_csv_backend_prefetch() {
        let mut csv = String::from("X,Y\n");
        for i in 0..100 {
            csv.push_str(&format!("a{},b{}\n", i, i));
        }
        let path = write_temp("csv", &csv);
        let mut table = load_file(&path).unwrap().into_iter().next().unwrap().1;

        table.prefetch_range(40, 20);
        // Rows 40..60 should all be in cache.
        for i in 40..60 {
            assert_eq!(table.get_row(i), &[format!("a{}", i), format!("b{}", i)]);
        }
        std::fs::remove_file(&path).ok();
    }
}
