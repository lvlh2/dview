use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;

use crate::colors::ColorPalette;
use crate::data::DataTable;
use crate::ui;

/// Viewport tracks scroll position and cursor within the data grid.
pub struct Viewport {
    /// Absolute cursor row (0-indexed into data.rows).
    pub cursor_row: usize,
    /// Absolute cursor column (0-indexed into data.headers).
    pub cursor_col: usize,
    /// First visible data row index.
    pub scroll_row: usize,
    /// First visible data column index.
    pub scroll_col: usize,
    /// How many data rows fit in the current viewport area.
    pub visible_rows: usize,
    /// How many data columns fit in the current viewport area.
    pub visible_cols: usize,
    /// Width of the row-number column in terminal cells.
    pub row_num_width: u16,
}

impl Viewport {
    pub fn new() -> Self {
        Self {
            cursor_row: 0,
            cursor_col: 0,
            scroll_row: 0,
            scroll_col: 0,
            visible_rows: 0,
            visible_cols: 0,
            row_num_width: 4,
        }
    }

    /// Recalculate visible dimensions based on terminal area and column widths.
    pub fn recalc_dimensions(
        &mut self,
        available_width: u16,
        available_height: u16,
        column_widths: &[usize],
        total_rows: usize,
    ) {
        // Row number column width.
        let digits = if total_rows == 0 {
            1
        } else {
            (total_rows as f64).log10() as usize + 1
        };
        self.row_num_width = (digits + 1).clamp(3, 8) as u16;

        // Only header row takes vertical space; rest are data rows.
        self.visible_rows = available_height.saturating_sub(1) as usize;

        // Available width: minus row number column + gap after it.
        const GAP: u16 = 3;
        let data_width = available_width.saturating_sub(self.row_num_width + GAP + 1);
        if data_width <= 2 {
            self.visible_cols = 0;
            return;
        }

        // Count how many columns fit (each column = width + gap).
        let mut used = 0u16;
        let mut count = 0usize;
        let start = self.scroll_col.min(column_widths.len().saturating_sub(1));
        for w in column_widths.iter().skip(start) {
            let col_w = *w as u16 + GAP;
            if used + col_w > data_width && count > 0 {
                break;
            }
            used += col_w;
            count += 1;
        }
        self.visible_cols = count.max(1);
    }

    // --- Cursor movement ---

    pub fn move_left(&mut self, _total_cols: usize) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
        self.ensure_col_visible();
    }

    pub fn move_right(&mut self, total_cols: usize) {
        if self.cursor_col + 1 < total_cols {
            self.cursor_col += 1;
        }
        self.ensure_col_visible();
    }

    pub fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
        self.ensure_row_visible();
    }

    pub fn move_down(&mut self, total_rows: usize) {
        if total_rows == 0 {
            return;
        }
        if self.cursor_row + 1 < total_rows {
            self.cursor_row += 1;
        }
        self.ensure_row_visible();
    }

    // --- Page navigation ---

    pub fn page_up(&mut self) {
        let delta = self.visible_rows.max(1);
        self.cursor_row = self.cursor_row.saturating_sub(delta);
        self.scroll_row = self.scroll_row.saturating_sub(delta);
        self.ensure_row_visible();
    }

    pub fn page_down(&mut self, total_rows: usize) {
        let delta = self.visible_rows.max(1);
        self.cursor_row = (self.cursor_row + delta).min(total_rows.saturating_sub(1));
        self.scroll_row =
            (self.scroll_row + delta).min(total_rows.saturating_sub(self.visible_rows));
        self.ensure_row_visible();
    }

    pub fn go_top(&mut self) {
        self.cursor_row = 0;
        self.scroll_row = 0;
    }

    pub fn go_bottom(&mut self, total_rows: usize) {
        if total_rows > 0 {
            self.cursor_row = total_rows - 1;
            self.scroll_row = total_rows.saturating_sub(self.visible_rows);
        }
    }

    // --- Column jump (0 / $) ---

    pub fn go_col_start(&mut self) {
        self.cursor_col = 0;
        self.scroll_col = 0;
    }

    pub fn go_col_end(&mut self, total_cols: usize) {
        if total_cols > 0 {
            self.cursor_col = total_cols - 1;
        }
        self.ensure_col_visible();
    }

    // --- View scroll (H/L horizontal, J/K vertical) ---
    // These move both the viewport AND the cursor so the cursor
    // stays at the same relative screen position.

    pub fn scroll_view_left(&mut self) {
        self.scroll_col = self.scroll_col.saturating_sub(1);
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
    }

    pub fn scroll_view_right(&mut self, total_cols: usize) {
        let max_scroll = total_cols.saturating_sub(self.visible_cols);
        if self.scroll_col < max_scroll {
            self.scroll_col += 1;
        }
        if self.cursor_col + 1 < total_cols {
            self.cursor_col += 1;
        }
    }

    pub fn scroll_view_up(&mut self) {
        self.scroll_row = self.scroll_row.saturating_sub(1);
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
    }

    pub fn scroll_view_down(&mut self, total_rows: usize) {
        let max_scroll = total_rows.saturating_sub(self.visible_rows);
        if self.scroll_row < max_scroll {
            self.scroll_row += 1;
        }
        if self.cursor_row + 1 < total_rows {
            self.cursor_row += 1;
        }
    }

    // --- Internal helpers ---

    fn ensure_row_visible(&mut self) {
        if self.cursor_row < self.scroll_row {
            self.scroll_row = self.cursor_row;
        }
        if self.visible_rows > 0 && self.cursor_row >= self.scroll_row + self.visible_rows {
            self.scroll_row = self.cursor_row - self.visible_rows + 1;
        }
    }

    fn ensure_col_visible(&mut self) {
        if self.cursor_col < self.scroll_col {
            self.scroll_col = self.cursor_col;
        }
        if self.visible_cols > 0 && self.cursor_col >= self.scroll_col + self.visible_cols {
            self.scroll_col = self.cursor_col - self.visible_cols + 1;
        }
    }
}

/// Main application state and event loop.
pub struct App {
    /// All loaded sheets (name, table).
    pub sheets: Vec<(String, DataTable)>,
    /// Index into self.sheets.
    pub active_sheet: usize,
    pub viewport: Viewport,
    pub palette: ColorPalette,
    pub running: bool,
    pub gg_pending: bool,
    pub show_help: bool,
    pub status_message: String,
}

impl App {
    pub fn new(sheets: Vec<(String, DataTable)>) -> Self {
        let data = sheets.first().map(|(_, t)| t);
        let status = if let Some(d) = data {
            format!(
                "{} rows × {} cols | ?:help  q:quit",
                d.total_rows(),
                d.total_cols()
            )
        } else {
            "? :help  q:quit".into()
        };
        Self {
            sheets,
            active_sheet: 0,
            viewport: Viewport::new(),
            palette: ColorPalette::default(),
            running: false,
            gg_pending: false,
            show_help: false,
            status_message: status,
        }
    }

    /// Reference to the currently active data table.
    pub fn data(&self) -> &DataTable {
        &self.sheets[self.active_sheet].1
    }

    /// Mutable reference to the currently active data table.
    pub fn data_mut(&mut self) -> &mut DataTable {
        &mut self.sheets[self.active_sheet].1
    }

    fn sheet_count(&self) -> usize {
        self.sheets.len()
    }

    fn switch_sheet(&mut self, delta: isize) {
        let n = self.sheet_count();
        if n <= 1 {
            return;
        }
        self.active_sheet = ((self.active_sheet as isize + delta + n as isize) as usize) % n;
        self.viewport = Viewport::new();
        let d = self.data();
        self.status_message = format!(
            "{} rows × {} cols | ?:help  q:quit",
            d.total_rows(),
            d.total_cols()
        );
    }

    pub fn run(
        &mut self,
        terminal: &mut Terminal<impl ratatui::backend::Backend>,
    ) -> anyhow::Result<()> {
        self.running = true;
        while self.running {
            // Prefetch visible rows before rendering (no-op for InMemory).
            if !self.show_help {
                let start = self.viewport.scroll_row;
                let count = self.viewport.visible_rows.max(1);
                self.data_mut().prefetch_range(start, count);
            }

            terminal
                .draw(|frame| ui::render(frame, self))
                .map_err(|e| anyhow::anyhow!("Draw error: {:?}", e))?;

            if event::poll(Duration::from_millis(50))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.handle_key(key);
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Help screen is modal — only Esc or ? dismiss it.
        if self.show_help {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') => self.show_help = false,
                _ => {}
            }
            return;
        }

        // Resolve gg double-press
        if self.gg_pending {
            self.gg_pending = false;
            if key.code == KeyCode::Char('g') {
                self.viewport.go_top();
                return;
            }
            // Not gg — fall through to normal handling.
        }

        // Ctrl+ combinations
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('f') => {
                    self.viewport.page_down(self.data().total_rows());
                }
                KeyCode::Char('b') => {
                    self.viewport.page_up();
                }
                KeyCode::Char('c') => {
                    self.running = false;
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.running = false,
            KeyCode::Char('?') => self.show_help = true,

            // Movement: hjkl
            KeyCode::Char('h') => {
                self.viewport.move_left(self.data().total_cols());
            }
            KeyCode::Char('j') => {
                self.viewport.move_down(self.data().total_rows());
            }
            KeyCode::Char('k') => {
                self.viewport.move_up();
            }
            KeyCode::Char('l') => {
                self.viewport.move_right(self.data().total_cols());
            }

            // View scroll: HJKL
            KeyCode::Char('H') => {
                self.viewport.scroll_view_left();
            }
            KeyCode::Char('L') => {
                self.viewport.scroll_view_right(self.data().total_cols());
            }
            KeyCode::Char('J') => {
                self.viewport.scroll_view_down(self.data().total_rows());
            }
            KeyCode::Char('K') => {
                self.viewport.scroll_view_up();
            }

            // View scroll: Shift+arrows (same as HJKL)
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.viewport.scroll_view_left();
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.viewport.scroll_view_right(self.data().total_cols());
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.viewport.scroll_view_up();
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.viewport.scroll_view_down(self.data().total_rows());
            }

            // Arrow keys: cursor movement (no modifiers)
            KeyCode::Left => {
                self.viewport.move_left(self.data().total_cols());
            }
            KeyCode::Right => {
                self.viewport.move_right(self.data().total_cols());
            }
            KeyCode::Up => {
                self.viewport.move_up();
            }
            KeyCode::Down => {
                self.viewport.move_down(self.data().total_rows());
            }

            // Sheet navigation: [ / ]
            KeyCode::Char('[') => self.switch_sheet(-1),
            KeyCode::Char(']') => self.switch_sheet(1),

            // Jump: g (first press of gg) / G (last row) / 0 (first col) / $ (last col)
            KeyCode::Char('g') => {
                self.gg_pending = true;
            }
            KeyCode::Char('G') => {
                self.viewport.go_bottom(self.data().total_rows());
            }
            KeyCode::Char('0') => {
                self.viewport.go_col_start();
            }
            KeyCode::Char('$') => {
                self.viewport.go_col_end(self.data().total_cols());
            }

            // Page up/down (without Ctrl)
            KeyCode::PageDown => {
                self.viewport.page_down(self.data().total_rows());
            }
            KeyCode::PageUp => {
                self.viewport.page_up();
            }
            KeyCode::Home => self.viewport.go_top(),
            KeyCode::End => {
                self.viewport.go_bottom(self.data().total_rows());
            }

            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataTable;

    fn make_table(rows: usize, cols: usize) -> DataTable {
        let headers: Vec<String> = (0..cols).map(|i| format!("Col{}", i)).collect();
        let data: Vec<Vec<String>> = (0..rows)
            .map(|r| (0..cols).map(|c| format!("R{}C{}", r, c)).collect())
            .collect();
        DataTable::new(headers, data)
    }

    // -------------------------------------------------------------------
    // Viewport defaults
    // -------------------------------------------------------------------

    #[test]
    fn test_viewport_defaults() {
        let vp = Viewport::new();
        assert_eq!(vp.cursor_row, 0);
        assert_eq!(vp.cursor_col, 0);
        assert_eq!(vp.scroll_row, 0);
        assert_eq!(vp.scroll_col, 0);
        assert_eq!(vp.visible_rows, 0);
        assert_eq!(vp.visible_cols, 0);
    }

    // -------------------------------------------------------------------
    // recalc_dimensions
    // -------------------------------------------------------------------

    #[test]
    fn test_recalc_dimensions_basic() {
        let mut vp = Viewport::new();
        // 80x24 terminal, 3 columns of width 5 each
        vp.recalc_dimensions(80, 24, &[5, 5, 5], 100);
        assert_eq!(vp.visible_rows, 23); // 24 - 1 header
        assert!(vp.visible_cols >= 3); // should fit with gaps
        assert!(vp.row_num_width >= 3);
    }

    #[test]
    fn test_recalc_dimensions_tiny_terminal() {
        let mut vp = Viewport::new();
        vp.recalc_dimensions(5, 3, &[5, 5], 10);
        // Very narrow: data_width will be 0 or small
        assert_eq!(vp.visible_rows, 2); // 3 - 1 header
        // visible_cols should not panic
    }

    #[test]
    fn test_recalc_dimensions_empty_data() {
        let mut vp = Viewport::new();
        vp.recalc_dimensions(80, 24, &[], 0);
        assert_eq!(vp.visible_rows, 23);
        // should not panic with empty column_widths
    }

    // -------------------------------------------------------------------
    // Cursor movement — basic
    // -------------------------------------------------------------------

    #[test]
    fn test_move_down_up() {
        let mut vp = Viewport::new();
        vp.visible_rows = 10;
        // move down
        vp.move_down(100);
        assert_eq!(vp.cursor_row, 1);
        // move up back
        vp.move_up();
        assert_eq!(vp.cursor_row, 0);
    }

    #[test]
    fn test_move_right_left() {
        let mut vp = Viewport::new();
        vp.visible_cols = 5;
        vp.move_right(20);
        assert_eq!(vp.cursor_col, 1);
        vp.move_left(20);
        assert_eq!(vp.cursor_col, 0);
    }

    // -------------------------------------------------------------------
    // Cursor movement — boundaries (no crash)
    // -------------------------------------------------------------------

    #[test]
    fn test_move_boundaries() {
        let mut vp = Viewport::new();
        vp.visible_rows = 5;
        vp.visible_cols = 3;

        // Can't move above row 0.
        vp.move_up();
        assert_eq!(vp.cursor_row, 0);

        // Can't move left of col 0.
        vp.move_left(10);
        assert_eq!(vp.cursor_col, 0);

        // Move to last row.
        vp.move_down(5); // 0→1
        vp.move_down(5); // 1→2
        vp.move_down(5); // 2→3
        vp.move_down(5); // 3→4
        vp.move_down(5); // 4→? can't go past 4
        assert_eq!(vp.cursor_row, 4);

        // One more should be clamped.
        vp.move_down(5);
        assert_eq!(vp.cursor_row, 4);

        // Move to last col.
        vp.move_right(3); // 0→1
        vp.move_right(3); // 1→2
        vp.move_right(3); // can't go past 2
        assert_eq!(vp.cursor_col, 2);
    }

    #[test]
    fn test_move_down_empty() {
        let mut vp = Viewport::new();
        vp.move_down(0); // total_rows = 0 — should not panic
        assert_eq!(vp.cursor_row, 0);
    }

    // -------------------------------------------------------------------
    // Page navigation
    // -------------------------------------------------------------------

    #[test]
    fn test_page_down() {
        let mut vp = Viewport::new();
        vp.visible_rows = 5;
        vp.page_down(30);
        assert_eq!(vp.cursor_row, 5);
        assert_eq!(vp.scroll_row, 5);

        // near end — clamps
        vp.cursor_row = 27;
        vp.scroll_row = 20;
        vp.visible_rows = 5;
        vp.page_down(30);
        assert_eq!(vp.cursor_row, 29); // clamped to last row
    }

    #[test]
    fn test_page_up() {
        let mut vp = Viewport::new();
        vp.cursor_row = 10;
        vp.scroll_row = 10;
        vp.visible_rows = 5;
        vp.page_up();
        assert_eq!(vp.cursor_row, 5);
        assert_eq!(vp.scroll_row, 5);

        // near top
        vp.cursor_row = 2;
        vp.scroll_row = 2;
        vp.page_up();
        assert_eq!(vp.cursor_row, 0);
        assert_eq!(vp.scroll_row, 0);
    }

    // -------------------------------------------------------------------
    // Jump to start / end
    // -------------------------------------------------------------------

    #[test]
    fn test_go_top() {
        let mut vp = Viewport::new();
        vp.cursor_row = 42;
        vp.scroll_row = 30;
        vp.go_top();
        assert_eq!(vp.cursor_row, 0);
        assert_eq!(vp.scroll_row, 0);
    }

    #[test]
    fn test_go_bottom() {
        let mut vp = Viewport::new();
        vp.visible_rows = 5;
        vp.go_bottom(100);
        assert_eq!(vp.cursor_row, 99);
        assert_eq!(vp.scroll_row, 95); // 100 - 5

        // empty data
        vp.go_bottom(0);
        assert_eq!(vp.cursor_row, 99); // unchanged
    }

    #[test]
    fn test_go_col_start() {
        let mut vp = Viewport::new();
        vp.cursor_col = 10;
        vp.scroll_col = 8;
        vp.go_col_start();
        assert_eq!(vp.cursor_col, 0);
        assert_eq!(vp.scroll_col, 0);
    }

    #[test]
    fn test_go_col_end() {
        let mut vp = Viewport::new();
        vp.visible_cols = 3;
        vp.go_col_end(20);
        assert_eq!(vp.cursor_col, 19);
        // scroll should be positioned so col 19 is visible.
        assert!(vp.cursor_col >= vp.scroll_col);
        assert!(vp.cursor_col < vp.scroll_col + vp.visible_cols);
    }

    #[test]
    fn test_go_col_end_empty() {
        let mut vp = Viewport::new();
        vp.go_col_end(0); // no columns — should not panic
        assert_eq!(vp.cursor_col, 0);
    }

    // -------------------------------------------------------------------
    // Horizontal view scroll (H / L)
    // -------------------------------------------------------------------

    #[test]
    fn test_scroll_view_left_right() {
        let mut vp = Viewport::new();
        vp.visible_cols = 3;
        vp.cursor_col = 5; // start cursor away from 0 so it can move

        vp.scroll_view_right(20);
        assert_eq!(vp.scroll_col, 1);
        assert_eq!(vp.cursor_col, 6); // cursor moves with view

        vp.scroll_view_left();
        assert_eq!(vp.scroll_col, 0);
        assert_eq!(vp.cursor_col, 5); // cursor moves back

        // Can't scroll left past 0.
        vp.cursor_col = 0;
        vp.scroll_col = 0;
        vp.scroll_view_left();
        assert_eq!(vp.scroll_col, 0);
        assert_eq!(vp.cursor_col, 0);

        // Scroll to the rightmost position.
        vp.cursor_col = 5;
        vp.scroll_col = 0;
        for _ in 0..30 {
            vp.scroll_view_right(20);
        }
        assert_eq!(vp.scroll_col, 17); // 20 - 3
        assert_eq!(vp.cursor_col, 19); // clamped at last col
    }

    #[test]
    fn test_scroll_view_up_down() {
        let mut vp = Viewport::new();
        vp.visible_rows = 5;
        vp.cursor_row = 10;

        vp.scroll_view_down(100);
        assert_eq!(vp.scroll_row, 1);
        assert_eq!(vp.cursor_row, 11); // cursor moves with view

        vp.scroll_view_up();
        assert_eq!(vp.scroll_row, 0);
        assert_eq!(vp.cursor_row, 10); // cursor moves back

        // Can't scroll up past 0.
        vp.cursor_row = 0;
        vp.scroll_row = 0;
        vp.scroll_view_up();
        assert_eq!(vp.scroll_row, 0);
        assert_eq!(vp.cursor_row, 0);

        // Scroll to bottom.
        vp.cursor_row = 10;
        vp.scroll_row = 0;
        for _ in 0..200 {
            vp.scroll_view_down(100);
        }
        assert_eq!(vp.scroll_row, 95); // 100 - 5
        assert_eq!(vp.cursor_row, 99); // clamped at last row
    }

    #[test]
    fn test_hjkl_scroll_moves_cursor_with_view() {
        // Cursor stays at the same relative screen position when scrolling.
        let mut vp = Viewport::new();
        vp.cursor_col = 5;
        vp.scroll_col = 3;
        vp.visible_cols = 5;

        // Cursor at relative screen col 2 (5 - 3).
        vp.scroll_view_right(20);
        // scroll_col=4, cursor_col=6 → relative screen col still 2.
        assert_eq!(vp.scroll_col, 4);
        assert_eq!(vp.cursor_col, 6);

        vp.scroll_view_left();
        assert_eq!(vp.scroll_col, 3);
        assert_eq!(vp.cursor_col, 5);
    }

    // -------------------------------------------------------------------
    // Ensure visible — auto-scroll with cursor
    // -------------------------------------------------------------------

    #[test]
    fn test_ensure_row_visible_on_move_down() {
        let mut vp = Viewport::new();
        vp.visible_rows = 5;
        // Move to row 6 (just beyond visible range starting at 0).
        for _ in 0..6 {
            vp.move_down(100);
        }
        assert_eq!(vp.cursor_row, 6);
        assert!(vp.scroll_row > 0); // viewport should have scrolled
    }

    #[test]
    fn test_ensure_col_visible_on_move_right() {
        let mut vp = Viewport::new();
        vp.visible_cols = 3;
        for _ in 0..5 {
            vp.move_right(20);
        }
        assert_eq!(vp.cursor_col, 5);
        assert!(vp.scroll_col > 0);
    }

    // -------------------------------------------------------------------
    // App key handling
    // -------------------------------------------------------------------

    fn make_app(rows: usize, cols: usize) -> App {
        App::new(vec![("Sheet1".into(), make_table(rows, cols))])
    }

    fn make_multi_sheet_app() -> App {
        let t1 = make_table(20, 5);
        let t2 = make_table(10, 8);
        let t3 = make_table(15, 3);
        App::new(vec![
            ("Data".into(), t1),
            ("Summary".into(), t2),
            ("Notes".into(), t3),
        ])
    }

    fn ctrl_key(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    fn press(ch: char) -> KeyEvent {
        KeyEvent::new_with_kind(KeyCode::Char(ch), KeyModifiers::NONE, KeyEventKind::Press)
    }

    #[test]
    fn test_handle_key_quit() {
        let mut app = make_app(10, 3);
        app.handle_key(press('q'));
        assert!(!app.running);
    }

    #[test]
    fn test_handle_key_esc() {
        let mut app = make_app(10, 3);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.running);
    }

    #[test]
    fn test_handle_key_hjkl() {
        let mut app = make_app(10, 5);
        app.viewport.visible_rows = 5;
        app.viewport.visible_cols = 3;

        app.handle_key(press('j'));
        assert_eq!(app.viewport.cursor_row, 1);

        app.handle_key(press('k'));
        assert_eq!(app.viewport.cursor_row, 0);

        app.handle_key(press('l'));
        assert_eq!(app.viewport.cursor_col, 1);

        app.handle_key(press('h'));
        assert_eq!(app.viewport.cursor_col, 0);
    }

    #[test]
    fn test_handle_key_arrow_keys() {
        let mut app = make_app(10, 5);
        app.viewport.visible_rows = 5;
        app.viewport.visible_cols = 3;

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.viewport.cursor_row, 1);

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.viewport.cursor_row, 0);

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.viewport.cursor_col, 1);

        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.viewport.cursor_col, 0);
    }

    #[test]
    fn test_shift_arrows_scroll_view() {
        let mut app = make_app(30, 20);
        app.viewport.visible_rows = 5;
        app.viewport.visible_cols = 3;
        app.viewport.cursor_row = 10;
        app.viewport.cursor_col = 10;

        let shift = KeyModifiers::SHIFT;

        // Shift+Right scrolls view right, cursor follows
        app.handle_key(KeyEvent::new(KeyCode::Right, shift));
        assert_eq!(app.viewport.scroll_col, 1);
        assert_eq!(app.viewport.cursor_col, 11);

        // Shift+Left scrolls view left
        app.handle_key(KeyEvent::new(KeyCode::Left, shift));
        assert_eq!(app.viewport.scroll_col, 0);
        assert_eq!(app.viewport.cursor_col, 10);

        // Shift+Down scrolls view down
        app.handle_key(KeyEvent::new(KeyCode::Down, shift));
        assert_eq!(app.viewport.scroll_row, 1);
        assert_eq!(app.viewport.cursor_row, 11);

        // Shift+Up scrolls view up
        app.handle_key(KeyEvent::new(KeyCode::Up, shift));
        assert_eq!(app.viewport.scroll_row, 0);
        assert_eq!(app.viewport.cursor_row, 10);
    }

    #[test]
    fn test_handle_key_ctrl_f_b() {
        let mut app = make_app(30, 3);
        app.viewport.visible_rows = 5;
        app.handle_key(ctrl_key('f'));
        assert_eq!(app.viewport.cursor_row, 5);

        app.handle_key(ctrl_key('b'));
        assert_eq!(app.viewport.cursor_row, 0);
    }

    #[test]
    fn test_ctrl_j_k_no_longer_page() {
        // Ctrl+J / Ctrl+K no longer trigger page down/up.
        let mut app = make_app(30, 3);
        app.viewport.visible_rows = 5;
        let row_before = app.viewport.cursor_row;
        app.handle_key(ctrl_key('j'));
        assert_eq!(app.viewport.cursor_row, row_before);
        app.handle_key(ctrl_key('k'));
        assert_eq!(app.viewport.cursor_row, row_before);
    }

    #[test]
    fn test_handle_key_gg_g() {
        let mut app = make_app(30, 3);
        app.viewport.visible_rows = 5;

        // gg (two g presses)
        app.viewport.cursor_row = 20;
        app.handle_key(press('g'));
        assert!(app.gg_pending);
        app.handle_key(press('g'));
        assert!(!app.gg_pending);
        assert_eq!(app.viewport.cursor_row, 0);

        // G
        app.handle_key(press('G'));
        assert_eq!(app.viewport.cursor_row, 29);
    }

    #[test]
    fn test_handle_key_gg_cancel() {
        let mut app = make_app(30, 3);
        app.viewport.visible_rows = 5;
        app.viewport.cursor_row = 10;

        // g then something else — gg pending cancelled, handles normally.
        app.handle_key(press('g'));
        assert!(app.gg_pending);
        app.handle_key(press('j')); // cancels gg, moves down instead
        assert!(!app.gg_pending);
        assert_eq!(app.viewport.cursor_row, 11); // moved down from 10
    }

    #[test]
    fn test_handle_key_0_dollar() {
        let mut app = make_app(10, 20);
        app.viewport.visible_cols = 5;

        app.viewport.cursor_col = 10;
        app.handle_key(press('0'));
        assert_eq!(app.viewport.cursor_col, 0);

        app.handle_key(press('$'));
        assert_eq!(app.viewport.cursor_col, 19);
    }

    #[test]
    fn test_handle_key_hjkl_scroll_view() {
        let mut app = make_app(30, 20);
        app.viewport.visible_cols = 3;
        app.viewport.visible_rows = 5;
        app.viewport.cursor_row = 10;
        app.viewport.cursor_col = 10;

        // L scrolls view right, cursor follows
        app.handle_key(press('L'));
        assert_eq!(app.viewport.scroll_col, 1);
        assert_eq!(app.viewport.cursor_col, 11);

        // H scrolls view left, cursor follows back
        app.handle_key(press('H'));
        assert_eq!(app.viewport.scroll_col, 0);
        assert_eq!(app.viewport.cursor_col, 10);

        // J scrolls view down, cursor follows
        app.handle_key(press('J'));
        assert_eq!(app.viewport.scroll_row, 1);
        assert_eq!(app.viewport.cursor_row, 11);

        // K scrolls view up, cursor follows back
        app.handle_key(press('K'));
        assert_eq!(app.viewport.scroll_row, 0);
        assert_eq!(app.viewport.cursor_row, 10);
    }

    #[test]
    fn test_handle_key_page_up_down() {
        let mut app = make_app(30, 3);
        app.viewport.visible_rows = 5;

        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(app.viewport.cursor_row, 5);

        app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(app.viewport.cursor_row, 0);
    }

    #[test]
    fn test_handle_key_home_end() {
        let mut app = make_app(30, 3);
        app.viewport.visible_rows = 5;
        app.viewport.cursor_row = 15;

        app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(app.viewport.cursor_row, 0);

        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(app.viewport.cursor_row, 29);
    }

    #[test]
    fn test_handle_key_ctrl_c_quit() {
        let mut app = make_app(10, 3);
        app.handle_key(ctrl_key('c'));
        assert!(!app.running);
    }

    #[test]
    fn test_handle_key_unknown_ignored() {
        let mut app = make_app(10, 3);
        let row_before = app.viewport.cursor_row;
        app.running = true;
        app.handle_key(press('x')); // unmapped key
        assert_eq!(app.viewport.cursor_row, row_before); // unchanged
        assert!(app.running); // still running
    }

    // -------------------------------------------------------------------
    // Help screen
    // -------------------------------------------------------------------

    #[test]
    fn test_help_toggle() {
        let mut app = make_app(10, 5);

        // ? opens help
        app.handle_key(press('?'));
        assert!(app.show_help);

        // ? again closes help
        app.handle_key(press('?'));
        assert!(!app.show_help);
    }

    #[test]
    fn test_esc_closes_help_does_not_quit() {
        let mut app = make_app(10, 5);
        app.running = true;
        app.show_help = true;

        // Esc should close help, NOT quit the app.
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.show_help);
        assert!(app.running);
    }

    #[test]
    fn test_keys_ignored_during_help() {
        let mut app = make_app(30, 20);
        app.viewport.visible_rows = 5;
        app.viewport.visible_cols = 3;
        app.show_help = true;
        let cursor_before = (app.viewport.cursor_row, app.viewport.cursor_col);

        // hjkl movement keys are ignored when help is shown.
        app.handle_key(press('j'));
        assert_eq!(app.viewport.cursor_row, cursor_before.0);

        app.handle_key(press('l'));
        assert_eq!(app.viewport.cursor_col, cursor_before.1);

        // H/L scroll keys are ignored.
        app.handle_key(press('L'));
        assert_eq!(app.viewport.scroll_col, 0);

        // q does NOT quit while help is shown.
        app.running = true;
        app.handle_key(press('q'));
        assert!(app.running);
        assert!(app.show_help);
    }

    // -------------------------------------------------------------------
    // Sheet navigation
    // -------------------------------------------------------------------

    #[test]
    fn test_sheet_switch_brackets() {
        let mut app = make_multi_sheet_app();
        assert_eq!(app.active_sheet, 0);

        // ] → next sheet
        app.handle_key(press(']'));
        assert_eq!(app.active_sheet, 1);

        // ] again → next
        app.handle_key(press(']'));
        assert_eq!(app.active_sheet, 2);

        // ] wraps around
        app.handle_key(press(']'));
        assert_eq!(app.active_sheet, 0);

        // [ wraps backward
        app.handle_key(press('['));
        assert_eq!(app.active_sheet, 2);
    }

    #[test]
    fn test_sheet_switch_resets_viewport() {
        let mut app = make_multi_sheet_app();
        app.viewport.cursor_row = 5;
        app.viewport.cursor_col = 3;
        app.viewport.scroll_row = 2;
        app.viewport.scroll_col = 1;

        app.handle_key(press(']'));
        assert_eq!(app.active_sheet, 1);
        // Viewport should be reset.
        assert_eq!(app.viewport.cursor_row, 0);
        assert_eq!(app.viewport.cursor_col, 0);
        assert_eq!(app.viewport.scroll_row, 0);
        assert_eq!(app.viewport.scroll_col, 0);
    }

    #[test]
    fn test_sheet_switch_single_sheet_noop() {
        // Single-sheet files: [ / ] are no-ops.
        let mut app = make_app(10, 5);
        assert_eq!(app.active_sheet, 0);

        app.handle_key(press(']'));
        assert_eq!(app.active_sheet, 0);

        app.handle_key(press('['));
        assert_eq!(app.active_sheet, 0);
    }
}
