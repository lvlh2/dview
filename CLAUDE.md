# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build / Test / Install

```bash
# Build (with proxy if in China)
export HTTPS_PROXY="http://127.0.0.1:7890"
cargo build
cargo run -- <file.csv>

# Run all tests (44 tests across data + app modules)
cargo test

# Run a single test
cargo test test_load_csv
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
| `data.rs` | `DataTable` struct + format loaders. Detect format by extension → dispatch to `load_csv`/`load_excel`/`load_parquet` |
| `app.rs` | `App` state + `Viewport` (cursor, scroll). Event loop, keybinding dispatch with `KeyEventKind::Press` filter |
| `ui.rs` | All rendering at `Buffer` level. Header row, data rows, status bar. No grid lines — columns separated by 3-space gaps |
| `colors.rs` | `ColorPalette` with 8-color rainbow cycle for columns, alternating row backgrounds, dark theme |

### Key design decisions

- **No grid lines**: Columns are separated by `COL_GAP = 3` spaces. Row separation relies entirely on alternating background colors (`row_bg_even`/`row_bg_odd`). There are no borders, no header separator, no vertical lines.
- **Cell text is not truncated**: Column widths are computed from the widest cell. `fit_text` returns the text as-is. Wide content requires horizontal scrolling (H/L keys).
- **CJK handling**: `put_text` uses `UnicodeWidthChar::width()` to advance cursor position (1 for ASCII, 2 for CJK). `compute_widths` in `data.rs` uses `UnicodeWidthStr::width()` for column width calculation.
- **HJKL view scroll**: `scroll_view_left`/`scroll_view_right`/`scroll_view_up`/`scroll_view_down` move both the viewport and the cursor in lockstep, so the cursor stays at the same relative screen position. `recalc_dimensions` must NOT call `ensure_col_visible()` or `ensure_row_visible()` — doing so would snap the viewport back to contain the cursor on every frame, undoing the scroll. Cursor-movement and jump functions each call their own `ensure_*_visible()` as needed.
- **Empty cell cursor**: The cell fill loop draws the column's background across the full `column_width` before drawing text, so cursor highlight remains visible on empty cells.

### Viewport model

- `cursor_row`/`cursor_col` — absolute position in the data grid (0-indexed)
- `scroll_row`/`scroll_col` — first visible row/column
- `visible_rows`/`visible_cols` — how many fit in the current terminal (recomputed each frame by `recalc_dimensions`)
- H/L move **only the view** (`scroll_col`), not the cursor. hjkl move the **cursor** with auto-scroll (`ensure_*_visible`).

### Keybinding reference

| Keys | Action |
|---|---|
| `h` `j` `k` `l` / arrows | Move cursor |
| `H` `L` `J` `K` | Scroll view left/right/up/down (cursor follows, stays at same screen position) |
| `Ctrl+F`/`Ctrl+J` `Ctrl+B`/`Ctrl+K` | Page down/up |
| `gg` (double-tap `g`) | Jump to first row |
| `G` | Jump to last row |
| `0` | Jump to first column |
| `$` | Jump to last column |
| `Home` | Jump to first row |
| `End` | Jump to last row |
| `PageUp` | Page up |
| `PageDown` | Page down |
| `q` `Esc` `Ctrl+C` | Quit |

### Data formats

Format detection is by file extension only. `load_file()` dispatches to:
- `.csv` → `csv` crate (comma delimiter)
- `.tsv` / `.tab` → `csv` crate (tab delimiter)
- `.xls` / `.xlsx` / `.xlsm` / `.xlsb` → `calamine` crate (first sheet)
- `.parquet` / `.pq` → `parquet` crate via `ParquetRecordBatchReaderBuilder` + arrow `RecordBatch`, with manual `arrow_value_to_str` downcast macro for common types

All cell values are converted to `String` on load. `DataTable.column_widths` are pre-computed from max content width (min 4, no upper bound).
