# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build / Test / Install

```bash
cargo build
cargo run -- <file.csv>

# Run all tests (55 tests)
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
| `data.rs` | `DataTable` struct + format loaders. `load_file()` returns `Vec<(String, DataTable)>` (sheet name + table). Format detection by extension; Excel loads all sheets via calamine. Also contains `cell_to_string` with Excel date serial → `YYYY-MM-DD` conversion. |
| `app.rs` | `App` state (sheets, active_sheet, viewport) + `Viewport` (cursor, scroll). Event loop, keybinding dispatch with `KeyEventKind::Press` filter. Help screen is a modal state. |
| `ui.rs` | All rendering at `Buffer` level. Header row, data rows, tab bar (multi-sheet), status bar, help screen. No grid lines — columns separated by 3-space gaps. |
| `colors.rs` | `ColorPalette` with 8-color rainbow cycle for columns, alternating row backgrounds, dark theme. |

### App state model

- `sheets: Vec<(String, DataTable)>` — all loaded sheets; access the active one via `app.data()` (returns `&DataTable`). Never use a stale `data` field — there is none.
- `active_sheet: usize` — index into `sheets`. Switched with `[`/`]` keys; viewport is reset on switch.
- `show_help: bool` — when true, a modal help screen is rendered instead of the data table. Only `Esc`/`?` are processed; all other keys are ignored.

### Key design decisions

- **No grid lines**: Columns are separated by `COL_GAP = 3` spaces. Row separation relies entirely on alternating background colors (`row_bg_even`/`row_bg_odd`). There are no borders, no header separator, no vertical lines.
- **Cell text is not truncated**: Column widths are computed from the widest cell (min 4, no upper bound). Wide content requires horizontal scrolling.
- **CJK handling**: `put_text` uses `UnicodeWidthChar::width()` to advance cursor position (1 for ASCII, 2 for CJK). `compute_widths` in `data.rs` uses `UnicodeWidthStr::width()` for column width calculation.
- **HJKL view scroll**: `scroll_view_*` methods move both the viewport AND the cursor in lockstep, so the cursor stays at the same relative screen position. `recalc_dimensions` must NOT call `ensure_col_visible()` or `ensure_row_visible()` — doing so would snap the viewport back to contain the cursor on every frame, undoing the scroll. Cursor-movement and jump functions each call their own `ensure_*_visible()` as needed.
- **Empty cell cursor**: The cell fill loop draws the column's background across the full `column_width` before drawing text, so cursor highlight remains visible on empty cells.
- **Excel dates**: `Data::DateTime(d)` is an `ExcelDateTime` (calamine 0.36). Use `d.as_f64()` for the serial number and `d.is_duration()` to distinguish dates from duration formats. `excel_serial_to_date()` handles the 1900 leap-year bug (serial 60 = fictional Feb 29).
- **Borrow splitting in ui.rs**: `app.data()` borrows all of `self`, so it cannot be called alongside `app.viewport.recalc_dimensions(...)`. Use inline field access (`&app.sheets[app.active_sheet].1`) instead.

### Viewport model

- `cursor_row`/`cursor_col` — absolute position in the data grid (0-indexed)
- `scroll_row`/`scroll_col` — first visible row/column
- `visible_rows`/`visible_cols` — how many fit in the current terminal (recomputed each frame by `recalc_dimensions`)
- `h`/`j`/`k`/`l` and arrow keys move the **cursor** with auto-scroll (`ensure_*_visible`).
- `H`/`J`/`K`/`L` and Shift+arrows scroll the **view**; the cursor moves with it so its relative screen position is preserved.

### Keybinding reference

| Keys | Action |
|---|---|
| `h` `j` `k` `l` / arrows | Move cursor |
| `H` `J` `K` `L` / Shift+arrows | Scroll view (cursor follows, stays at same screen position) |
| `Ctrl+F` `Ctrl+B` | Page down/up |
| `gg` (double-tap `g`) | Jump to first row |
| `G` | Jump to last row |
| `0` | Jump to first column |
| `$` | Jump to last column |
| `Home` / `End` | Jump to first / last row |
| `PageUp` / `PageDown` | Page up / down |
| `[` `]` | Previous / next sheet (multi-sheet Excel only) |
| `?` | Toggle help screen (Esc to close) |
| `q` `Esc` `Ctrl+C` | Quit |

### Data formats

Format detection is by file extension only. `load_file()` returns `Vec<(String, DataTable)>` and dispatches to:
- `.csv` → `csv` crate (comma delimiter)
- `.tsv` / `.tab` → `csv` crate (tab delimiter)
- `.xls` / `.xlsx` / `.xlsm` / `.xlsb` → `calamine` crate, all sheets loaded, each first row = headers
- `.parquet` / `.pq` → `parquet` crate via `ParquetRecordBatchReaderBuilder` + arrow `RecordBatch`, with manual `arrow_value_to_str` downcast macro for common types

All cell values are converted to `String` on load. For CSV/TSV/Parquet the sheet name is the file stem; for Excel it's the worksheet name.
