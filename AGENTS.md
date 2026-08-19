# dview Agent Notes

## Commands

- Requires Rust 1.85+ (edition 2024).
- Format and test: `cargo fmt && cargo test`. Unit tests live beside their modules in `src/`.
- Run a focused test with `cargo test data::tests::<test_name>` or `cargo test app::tests::<test_name>`.
- `cargo run -- <file>` opens an interactive raw-mode alternate-screen TUI; use it only for manual terminal verification.

## Structure

- This is a single binary crate. `main.rs` owns CLI parsing and terminal setup; `data.rs` loads all file formats; `app.rs` owns state, input, and the event loop; `ui.rs` renders directly to a `ratatui::Buffer`.
- `load_file()` returns all sheets as `Vec<(String, DataTable)>`; Excel is multi-sheet and in-memory, while CSV/TSV and Parquet use lazy backends.

## Data And Rendering Constraints

- Rows may be lazily loaded. Use `DataTable::get_row()` rather than accessing backend storage, and preserve the event loop's `prefetch_range()` before each draw.
- CSV readers deliberately use `flexible(true)`: ragged rows must load. Extra cells are not rendered beyond the header count. The UTF-8 probe is only for encoding detection; parsing and byte offsets must start at byte zero.
- Keep Unicode display widths correct: compute widths with `UnicodeWidthStr`, and render all cell text through `ui::put_text()` rather than `Buffer::set_char` loops.
- Parquet string conversion treats null as an empty string and supports Arrow temporal types. Excel serial dates preserve the 1900 leap-year compatibility case.

## Interaction Invariants

- Lowercase `hjkl`/arrows move the cursor and auto-scroll. Uppercase `HJKL`/Shift+arrows scroll the viewport and cursor together; do not re-run `ensure_*_visible()` from dimension recalculation.
- With the help modal open, only `Esc` and `?` are handled; `q` must not exit. Switching sheets with `[`/`]` resets the viewport.
