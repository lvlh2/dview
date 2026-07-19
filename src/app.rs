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
        self.row_num_width = (digits + 1).max(3).min(8) as u16;

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
        self.scroll_row = (self.scroll_row + delta).min(total_rows.saturating_sub(self.visible_rows));
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

    // --- Horizontal view scroll (H/L keys, moves view NOT cursor) ---

    pub fn scroll_view_left(&mut self) {
        self.scroll_col = self.scroll_col.saturating_sub(1);
    }

    pub fn scroll_view_right(&mut self, total_cols: usize) {
        let max_scroll = total_cols.saturating_sub(self.visible_cols);
        if self.scroll_col < max_scroll {
            self.scroll_col += 1;
        }
    }

    // --- Internal helpers ---

    fn ensure_row_visible(&mut self) {
        if self.cursor_row < self.scroll_row {
            self.scroll_row = self.cursor_row;
        }
        if self.visible_rows > 0
            && self.cursor_row >= self.scroll_row + self.visible_rows
        {
            self.scroll_row = self.cursor_row - self.visible_rows + 1;
        }
    }

    fn ensure_col_visible(&mut self) {
        if self.cursor_col < self.scroll_col {
            self.scroll_col = self.cursor_col;
        }
        if self.visible_cols > 0
            && self.cursor_col >= self.scroll_col + self.visible_cols
        {
            self.scroll_col = self.cursor_col - self.visible_cols + 1;
        }
    }
}

/// Main application state and event loop.
pub struct App {
    pub data: DataTable,
    pub viewport: Viewport,
    pub palette: ColorPalette,
    pub running: bool,
    pub gg_pending: bool,
    pub status_message: String,
}

impl App {
    pub fn new(data: DataTable) -> Self {
        let status = format!(
            "{} rows × {} cols | q:quit  hjkl:move  H/L:scroll view  0/$:col start/end  gg/G:row start/end  Ctrl+F/B:page down/up",
            data.total_rows(),
            data.total_cols()
        );
        Self {
            data,
            viewport: Viewport::new(),
            palette: ColorPalette::default(),
            running: false,
            gg_pending: false,
            status_message: status,
        }
    }

    pub fn run(&mut self, terminal: &mut Terminal<impl ratatui::backend::Backend>) -> anyhow::Result<()> {
        self.running = true;
        while self.running {
            terminal
                .draw(|frame| ui::render(frame, self))
                .map_err(|e| anyhow::anyhow!("Draw error: {:?}", e))?;

            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key(key);
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
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
                KeyCode::Char('f') | KeyCode::Char('j') => {
                    self.viewport.page_down(self.data.total_rows());
                }
                KeyCode::Char('b') | KeyCode::Char('k') => {
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

            // Movement: hjkl
            KeyCode::Char('h') | KeyCode::Left => {
                self.viewport.move_left(self.data.total_cols());
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.viewport.move_down(self.data.total_rows());
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.viewport.move_up();
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.viewport.move_right(self.data.total_cols());
            }

            // Horizontal view scroll: H / L
            KeyCode::Char('H') => {
                self.viewport.scroll_view_left();
            }
            KeyCode::Char('L') => {
                self.viewport.scroll_view_right(self.data.total_cols());
            }

            // Jump: g (first press of gg) / G (last row) / 0 (first col) / $ (last col)
            KeyCode::Char('g') => {
                self.gg_pending = true;
            }
            KeyCode::Char('G') => {
                self.viewport.go_bottom(self.data.total_rows());
            }
            KeyCode::Char('0') => {
                self.viewport.go_col_start();
            }
            KeyCode::Char('$') => {
                self.viewport.go_col_end(self.data.total_cols());
            }

            // Page up/down (without Ctrl)
            KeyCode::PageDown => {
                self.viewport.page_down(self.data.total_rows());
            }
            KeyCode::PageUp => {
                self.viewport.page_up();
            }
            KeyCode::Home => self.viewport.go_top(),
            KeyCode::End => {
                self.viewport.go_bottom(self.data.total_rows());
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

        vp.scroll_view_right(20);
        assert_eq!(vp.scroll_col, 1);

        vp.scroll_view_left();
        assert_eq!(vp.scroll_col, 0);

        // Can't scroll left past 0.
        vp.scroll_view_left();
        assert_eq!(vp.scroll_col, 0);

        // Scroll to the rightmost position.
        for _ in 0..30 {
            vp.scroll_view_right(20);
        }
        assert_eq!(vp.scroll_col, 17); // 20 - 3
    }

    #[test]
    fn test_h_l_do_not_move_cursor() {
        let mut vp = Viewport::new();
        vp.cursor_col = 5;
        vp.visible_cols = 3;
        vp.scroll_view_right(20);
        assert_eq!(vp.scroll_col, 1);
        assert_eq!(vp.cursor_col, 5); // cursor unchanged
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
        App::new(make_table(rows, cols))
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
    fn test_handle_key_ctrl_f_b() {
        let mut app = make_app(30, 3);
        app.viewport.visible_rows = 5;
        app.handle_key(ctrl_key('f'));
        assert_eq!(app.viewport.cursor_row, 5);

        app.handle_key(ctrl_key('b'));
        assert_eq!(app.viewport.cursor_row, 0);
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
    fn test_handle_key_h_l_scroll() {
        let mut app = make_app(10, 20);
        app.viewport.visible_cols = 3;

        app.handle_key(press('L'));
        assert_eq!(app.viewport.scroll_col, 1);

        app.handle_key(press('H'));
        assert_eq!(app.viewport.scroll_col, 0);
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
}
