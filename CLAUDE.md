# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build / Test / Install

```bash
cargo build
cargo run -- <file.csv>

# Run all tests (61 tests)
cargo test

# Run a single test
cargo test test_excel_serial_date_conversion
cargo test test_handle_key_hjkl

# Install globally (~/.cargo/bin/dview)
cargo install --path .
```

## Architecture

`dview` is a terminal data viewer that opens CSV/TSV/Excel/Parquet files in a TUI with vim-style navigation. It uses **ratatui + crossterm** for the TUI and renders cells manually at the `Buffer` level (no built-in Table widget).

### Module responsibilities

| Module | Purpose |
|---|---|
| `main.rs` | CLI arg parsing (`clap`), terminal raw-mode init, panic hook for terminal restore |
| `data.rs` | `DataTable` + `RowBackend` enum (InMemory/Csv/Parquet). Format loaders, `arrow_value_to_str` for Parquet, Excel date serial conversion |
| `app.rs` | `App` state + `Viewport` (cursor, scroll). Event loop with `prefetch_range` before each frame. Keybinding dispatch. Help modal state |
| `ui.rs` | All rendering at `Buffer` level. `render_table` takes `&mut DataTable` + `&Viewport` separately for borrow splitting |
| `colors.rs` | `ColorPalette` with 8-color rainbow cycle for columns, alternating row backgrounds, dark theme |

### App state model

- `sheets: Vec<(String, DataTable)>` — all loaded sheets; access via `app.data()` (`&DataTable`) or `app.data_mut()` (`&mut DataTable`).
- `active_sheet: usize` — index into `sheets`. Switched with `[`/`]` keys; viewport is reset on switch.
- `show_help: bool` — when true, a modal help screen is rendered. Only `Esc`/`?` are processed; all other keys are ignored.

### DataTable and RowBackend — large file support

`DataTable` stores `headers` and `column_widths` in memory. Rows are handled by a private `RowBackend` enum:

- **`RowBackend::InMemory(Vec<Vec<String>>)`** — Excel files, small CSV/Parquet, and all tests. `get_row(idx)` returns `&rows[idx]`. `prefetch_range` is a no-op.
- **`RowBackend::Csv(CsvBackend)`** — Byte-offset indexed. On open: sequential pass builds `Vec<u64>` offset index (8 bytes × N rows) and sampled column widths. On `prefetch_range`/`get_row`: seeks `BufReader<File>` to the byte offset, reads a window (~500 rows), parses with `csv::Reader`. Only the cached window is in memory.
- **`RowBackend::Parquet(ParquetBackend)`** — Row-group lazy loading. On open: reads metadata only (schema + row group boundaries), samples first row group for widths. On `prefetch_range`/`get_row`: uses `with_row_selection(RowSelection)` to read only the target rows (±200 margin) from the relevant row group, avoiding full row group decompression.

Key API:
- `total_rows()` / `total_cols()` — `&self`, used everywhere in `handle_key`
- `get_row(&mut self, idx) -> &[String]` — returns reference into internal cache; for lazy backends may read from disk
- `prefetch_range(&mut self, start, count)` — called once per frame in the event loop (before `terminal.draw()`) to warm the cache for the visible viewport

**Never access `.rows` directly** — there is no public `rows` field. Use `get_row()`.

### Key design decisions

- **No grid lines**: Columns are separated by `COL_GAP = 3` spaces. Row separation relies entirely on alternating background colors (`row_bg_even`/`row_bg_odd`). No borders, no header separator, no vertical lines.
- **Cell text is not truncated**: Column widths are computed from the widest cell (min 4, no upper bound). Wide content requires horizontal scrolling.
- **CJK handling**: `put_text` uses `UnicodeWidthChar::width()` to advance cursor position (1 for ASCII, 2 for CJK). Width computation uses `UnicodeWidthStr::width()`.
- **HJKL view scroll**: `scroll_view_*` methods move both the viewport AND the cursor in lockstep. `recalc_dimensions` must NOT call `ensure_*_visible()` — that would snap the viewport back to contain the cursor on every frame.
- **Borrow splitting in ui.rs**: `render_table` receives `&mut DataTable` and `&Viewport` as separate parameters. `vis_col_widths` is extracted into an owned `Vec<u16>` before the data row loop to avoid borrowing `data` immutably while `get_row()`'s result is live. `compute_layout` takes individual params instead of `&App`.
- **Column width estimation**: For CSV, widths are sampled every K-th row during index building (K ≈ total_bytes/20/2000). For Parquet, first row group is sampled (up to 2000 rows). Both use `min(4)` floor. `refine_widths()` exists for opportunistic refinement during scroll but is not yet wired in.
- **Excel dates**: `Data::DateTime(d)` (calamine 0.36). Use `d.as_f64()` for serial number, `d.is_duration()` for duration vs date. `excel_serial_to_date()` handles the 1900 leap-year bug.
- **Event loop**: `prefetch_range(scroll_row, visible_rows)` is called before each `terminal.draw()`. This ensures the cache covers the current viewport before rendering. For InMemory this is a no-op.

### Viewport model

- `cursor_row`/`cursor_col` — absolute position in data grid (0-indexed)
- `scroll_row`/`scroll_col` — first visible row/column
- `visible_rows`/`visible_cols` — how many fit in current terminal (recomputed each frame)
- `h`/`j`/`k`/`l` / arrows move **cursor** with auto-scroll (`ensure_*_visible`)
- `H`/`J`/`K`/`L` / Shift+arrows scroll the **view**; cursor moves with it

### Keybinding reference

| Keys | Action |
|---|---|
| `h` `j` `k` `l` / arrows | Move cursor |
| `H` `J` `K` `L` / Shift+arrows | Scroll view |
| `Ctrl+F` `Ctrl+B` | Page down/up |
| `gg` (double-tap `g`) | Jump to first row |
| `G` | Jump to last row |
| `0` | Jump to first column |
| `$` | Jump to last column |
| `Home` / `End` | Jump to first / last row |
| `PageUp` / `PageDown` | Page up / down |
| `[` `]` | Previous / next sheet |
| `?` | Toggle help screen |
| `q` `Esc` `Ctrl+C` | Quit |

### Data formats

Format detection by file extension. `load_file()` returns `Vec<(String, DataTable)>`:

| Extensions | Loader | Backend |
|---|---|---|
| `.csv` | `csv` crate, comma delimiter | `CsvBackend` |
| `.tsv` / `.tab` | `csv` crate, tab delimiter | `CsvBackend` |
| `.xls` / `.xlsx` / `.xlsm` / `.xlsb` | `calamine` crate, all sheets | `InMemory` |
| `.parquet` / `.pq` | `parquet` crate via `ParquetRecordBatchReaderBuilder` | `ParquetBackend` |

Sheet names: file stem for CSV/TSV/Parquet; worksheet name for Excel. Excel always loads into memory (no lazy support — calamine doesn't stream).
