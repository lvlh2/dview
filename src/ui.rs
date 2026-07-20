use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::app::App;
use crate::colors::ColorPalette;

/// Number of spaces between adjacent columns.
const COL_GAP: u16 = 3;

fn rect_right(area: Rect) -> u16 {
    area.x + area.width
}
fn rect_bottom(area: Rect) -> u16 {
    area.y + area.height
}

/// Render one frame.
pub fn render(frame: &mut ratatui::Frame, app: &mut App) {
    let area = frame.area();
    let palette = &app.palette;
    let buf = frame.buffer_mut();

    if app.show_help {
        render_help_screen(buf, area, palette);
        return;
    }

    let tab_bar_height = if app.sheets.len() > 1 { 1u16 } else { 0u16 };
    let status_height = 1;
    let table_area = Rect::new(
        area.x,
        area.y,
        area.width,
        area.height.saturating_sub(status_height + tab_bar_height),
    );

    // Use inline field access so Rust can split borrows (app.sheets vs app.viewport).
    let active_data = &app.sheets[app.active_sheet].1;
    app.viewport.recalc_dimensions(
        table_area.width,
        table_area.height,
        &active_data.column_widths,
        active_data.total_rows(),
    );

    // Fill entire background.
    for y in table_area.y..rect_bottom(table_area) {
        for x in table_area.x..rect_right(table_area) {
            buf[(x, y)].set_char(' ').set_bg(palette.bg);
        }
    }

    if app.data().total_rows() > 0 || app.data().total_cols() > 0 {
        render_table(buf, table_area, app, palette);
    }

    // Tab bar (multi-sheet Excel only).
    if tab_bar_height > 0 {
        let tab_y = rect_bottom(table_area);
        render_tab_bar(buf, area, tab_y, app, palette);
    }

    render_status_bar(buf, area, app, palette);
}

// ---------------------------------------------------------------------------
// Column position calculation
// ---------------------------------------------------------------------------

struct ColLayout {
    /// x position of each visible column's text (including row-num as column -1).
    xs: Vec<u16>,
    /// The visible column indices.
    vis_cols: Vec<usize>,
    /// Width of the row-number column.
    row_num_x: u16,
    row_num_w: u16,
    /// Number of visible data rows.
    visible_rows: usize,
}

fn compute_layout(area: Rect, app: &App) -> ColLayout {
    let vp = &app.viewport;
    let total_cols = app.data().total_cols();

    let vis_cols: Vec<usize> =
        (vp.scroll_col..total_cols.min(vp.scroll_col + vp.visible_cols)).collect();

    // Row number position.
    let row_num_x = area.x + 1;
    let row_num_w = vp.row_num_width.max(3);

    // Data column positions.
    let mut xs: Vec<u16> = Vec::with_capacity(vis_cols.len());
    let mut x = row_num_x + row_num_w + COL_GAP;
    for &ci in &vis_cols {
        if x + app.data().column_widths.get(ci).copied().unwrap_or(4) as u16 > rect_right(area) {
            break;
        }
        xs.push(x);
        x += app.data().column_widths.get(ci).copied().unwrap_or(4) as u16 + COL_GAP;
    }

    let visible_rows = (rect_bottom(area).saturating_sub(area.y + 1)) as usize;

    let vis_cols = vis_cols[..xs.len()].to_vec(); // trim to what fits

    ColLayout {
        xs,
        vis_cols,
        row_num_x,
        row_num_w,
        visible_rows,
    }
}

// ---------------------------------------------------------------------------
// Table rendering
// ---------------------------------------------------------------------------

fn render_table(buf: &mut Buffer, area: Rect, app: &App, palette: &ColorPalette) {
    let layout = compute_layout(area, app);
    let vp = &app.viewport;

    // --- Header row ---
    let hdr_y = area.y;
    // Fill header background across the full width.
    for y_off in 0..1u16 {
        for xi in area.x..rect_right(area) {
            buf[(xi, hdr_y + y_off)]
                .set_char(' ')
                .set_bg(palette.header_bg);
        }
    }

    // Row number header.
    put_text(
        buf,
        layout.row_num_x,
        hdr_y,
        &format!("{:>width$}", "#", width = layout.row_num_w as usize),
        Style::new().fg(palette.header_fg).bg(palette.header_bg),
    );

    // Column headers.
    for (vi, &ci) in layout.vis_cols.iter().enumerate() {
        let col_fg = palette.column_color(ci);
        let style = Style::new().fg(col_fg).bg(palette.header_bg);
        let text = &app.data().headers[ci];
        put_text(buf, layout.xs[vi], hdr_y, text, style);
    }

    // --- Data rows ---
    let data_start_y = area.y + 1;
    for i in 0..layout.visible_rows {
        let data_row = vp.scroll_row + i;
        let screen_y = data_start_y + i as u16;

        if screen_y >= rect_bottom(area) {
            break;
        }
        if data_row >= app.data().total_rows() {
            break;
        }

        let bg = if data_row % 2 == 0 {
            palette.row_bg_even
        } else {
            palette.row_bg_odd
        };
        let is_cursor_row = data_row == vp.cursor_row;

        // Fill row background.
        for xi in area.x..rect_right(area) {
            buf[(xi, screen_y)].set_char(' ').set_bg(bg);
        }

        // Row number.
        let rn_style = if is_cursor_row {
            Style::new().fg(palette.cursor_fg).bg(palette.cursor_bg)
        } else {
            Style::new().fg(palette.row_num_fg).bg(bg)
        };
        put_text(
            buf,
            layout.row_num_x,
            screen_y,
            &format!("{:>width$}", data_row + 1, width = layout.row_num_w as usize),
            rn_style,
        );

        // Data cells.
        let row = &app.data().rows[data_row];
        for (vi, &ci) in layout.vis_cols.iter().enumerate() {
            let col_fg = palette.column_color(ci);
            let is_cursor_cell = is_cursor_row && ci == vp.cursor_col;
            let col_w = app.data().column_widths.get(ci).copied().unwrap_or(4) as u16;

            let cell_style = if is_cursor_cell {
                Style::new().fg(palette.cursor_fg).bg(palette.cursor_bg)
            } else {
                Style::new().fg(col_fg).bg(bg)
            };

            // Fill full column width with the cell's background so the cursor
            // remains visible even on empty cells.
            for ox in 0..col_w {
                let cx = layout.xs[vi] + ox;
                if cx < rect_right(*buf.area()) {
                    buf[(cx, screen_y)]
                        .set_char(' ')
                        .set_bg(cell_style.bg.unwrap_or(bg));
                }
            }

            let cell_text = row.get(ci).map(|s| s.as_str()).unwrap_or("");
            put_text(buf, layout.xs[vi], screen_y, cell_text, cell_style);
        }
    }
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn render_status_bar(buf: &mut Buffer, area: Rect, app: &App, palette: &ColorPalette) {
    let y = rect_bottom(area).saturating_sub(1);
    let style = Style::new().fg(palette.row_num_fg).bg(palette.header_bg);

    for xi in area.x..rect_right(area) {
        buf[(xi, y)].set_char(' ').set_bg(palette.header_bg);
    }

    let pos_info = if app.data().total_rows() > 0 {
        format!(
            " Row {}/{}  Col {}/{} ",
            app.viewport.cursor_row + 1,
            app.data().total_rows(),
            app.viewport.cursor_col + 1,
            app.data().total_cols(),
        )
    } else {
        String::new()
    };

    let display: String = format!("{}{}", pos_info, app.status_message)
        .chars()
        .take(area.width as usize)
        .collect();

    for (i, ch) in display.chars().enumerate() {
        let cx = area.x + i as u16;
        if cx < rect_right(area) {
            buf[(cx, y)].set_char(ch).set_style(style);
        }
    }
}

// ---------------------------------------------------------------------------
// Tab bar (multi-sheet Excel)
// ---------------------------------------------------------------------------

fn render_tab_bar(buf: &mut Buffer, area: Rect, y: u16, app: &App, palette: &ColorPalette) {
    // Fill background.
    for x in area.x..rect_right(area) {
        buf[(x, y)].set_char(' ').set_bg(palette.header_bg);
    }

    // "[  sheet1  |  sheet2  |  sheet3  ]"
    let active_style = Style::new()
        .fg(palette.cursor_fg)
        .bg(palette.cursor_bg);
    let inactive_style = Style::new().fg(palette.header_fg).bg(palette.header_bg);
    let bracket_style = Style::new().fg(palette.row_num_fg).bg(palette.header_bg);
    let sep_style = Style::new().fg(palette.row_num_fg).bg(palette.header_bg);

    let mut cx = area.x + 2; // left margin

    // Left bracket.
    buf[(cx, y)].set_char('[').set_style(bracket_style);
    cx += 1;

    for (i, (name, _)) in app.sheets.iter().enumerate() {
        // Separator between tabs.
        if i > 0 {
            buf[(cx, y)].set_char('│').set_style(sep_style);
            cx += 1;
        }

        let style = if i == app.active_sheet {
            active_style
        } else {
            inactive_style
        };

        // Pad the tab name.
        let label = format!(" {} ", name);
        for ch in label.chars() {
            if cx + 1 >= rect_right(area) {
                break;
            }
            buf[(cx, y)].set_char(ch).set_style(style);
            cx += 1;
        }
    }

    // Right bracket.
    if cx < rect_right(area) {
        buf[(cx, y)].set_char(']').set_style(bracket_style);
    }
}

// ---------------------------------------------------------------------------
// Help screen
// ---------------------------------------------------------------------------

fn render_help_screen(buf: &mut Buffer, area: Rect, palette: &ColorPalette) {
    // Fill background.
    for y in area.y..rect_bottom(area) {
        for x in area.x..rect_right(area) {
            buf[(x, y)].set_char(' ').set_bg(palette.header_bg);
        }
    }

    let fg = palette.header_fg;
    let bg = palette.header_bg;

    // Help content: (label, description) pairs.  A label of "" is a section header.
    let lines: &[(&str, &str)] = &[
        ("", "Navigation"),
        ("h / Left", "Move cursor left one column"),
        ("j / Down", "Move cursor down one row"),
        ("k / Up", "Move cursor up one row"),
        ("l / Right", "Move cursor right one column"),
        ("", ""),
        ("", "View Scroll  (cursor moves with view)"),
        ("H / Shift+Left", "Scroll view left one column"),
        ("J / Shift+Down", "Scroll view down one row"),
        ("K / Shift+Up", "Scroll view up one row"),
        ("L / Shift+Right", "Scroll view right one column"),
        ("", ""),
        ("", "Jump"),
        ("gg", "Jump to first row  (press g twice)"),
        ("G", "Jump to last row"),
        ("0", "Jump to first column"),
        ("$", "Jump to last column"),
        ("Home", "Jump to first row"),
        ("End", "Jump to last row"),
        ("", ""),
        ("", "Page"),
        ("Ctrl+F", "Page down"),
        ("Ctrl+B", "Page up"),
        ("PageDown", "Page down"),
        ("PageUp", "Page up"),
        ("", ""),
        ("", "Sheet  (multi-sheet Excel only)"),
        ("[ / ]", "Previous / next sheet"),
        ("", ""),
        ("", "Other"),
        ("?", "Show / hide this help screen"),
        ("Esc / q", "Quit dview"),
        ("Ctrl+C", "Quit dview"),
    ];

    let footer = " Press Esc or ? to close help ";

    // Box dimensions.
    let label_width: u16 = 18;
    let gap: u16 = 3;
    let content_width: u16 = label_width + gap + 36; // 36 for description
    let inner_w: u16 = content_width;
    let inner_h: u16 = lines.len() as u16 + 3; // +2 blank rows + footer

    // Clamp to terminal.
    let box_w = (inner_w + 2).min(area.width.saturating_sub(2));
    let box_h = (inner_h + 2).min(area.height.saturating_sub(2));
    let ox = area.x + (area.width.saturating_sub(box_w)) / 2;
    let oy = area.y + (area.height.saturating_sub(box_h)) / 2;

    let style = Style::new().fg(fg).bg(bg);

    // Draw border.
    // Top edge.
    buf[(ox, oy)].set_char('┌').set_style(style);
    for x in ox + 1..ox + box_w - 1 {
        buf[(x, oy)].set_char('─').set_style(style);
    }
    buf[(ox + box_w - 1, oy)].set_char('┐').set_style(style);

    // Bottom edge.
    let by = oy + box_h - 1;
    buf[(ox, by)].set_char('└').set_style(style);
    for x in ox + 1..ox + box_w - 1 {
        buf[(x, by)].set_char('─').set_style(style);
    }
    buf[(ox + box_w - 1, by)].set_char('┘').set_style(style);

    // Side edges.
    for y in oy + 1..by {
        buf[(ox, y)].set_char('│').set_style(style);
        buf[(ox + box_w - 1, y)].set_char('│').set_style(style);
    }

    // Title.
    let title = " Help ";
    let tx = ox + 2;
    for (i, ch) in title.chars().enumerate() {
        let cx = tx + i as u16;
        if cx < ox + box_w - 1 {
            buf[(cx, oy)].set_char(ch).set_style(style);
        }
    }

    // Render content lines.
    let content_x = ox + 2;
    let content_y = oy + 1;
    let desc_x = content_x + label_width + gap;

    for (li, (label, desc)) in lines.iter().enumerate() {
        let y = content_y + li as u16;
        if y >= oy + box_h - 1 {
            break;
        }

        if label.is_empty() && desc.is_empty() {
            // Blank separator line.
            continue;
        }

        if label.is_empty() {
            // Section header.
            let hdr_style = Style::new().fg(palette.row_num_fg).bg(bg);
            for (i, ch) in desc.chars().enumerate() {
                let cx = content_x + i as u16;
                if cx < ox + box_w - 1 {
                    buf[(cx, y)].set_char(ch).set_style(hdr_style);
                }
            }
        } else {
            // Key label — right-aligned within label_width.
            let label_style = Style::new().fg(palette.column_color(0)).bg(bg);
            let padding = label_width as usize - label.len();
            for (i, ch) in format!("{:>width$}", label, width = label.len() + padding).chars().enumerate() {
                let cx = content_x + i as u16;
                if cx < ox + box_w - 1 {
                    buf[(cx, y)].set_char(ch).set_style(label_style);
                }
            }
            // Description.
            let desc_style = Style::new().fg(fg).bg(bg);
            for (i, ch) in desc.chars().enumerate() {
                let cx = desc_x + i as u16;
                if cx < ox + box_w - 1 {
                    buf[(cx, y)].set_char(ch).set_style(desc_style);
                }
            }
        }
    }

    // Footer.
    let fy = oy + box_h - 2;
    let footer_pad = (box_w as usize - 2).saturating_sub(footer.len()) / 2;
    let footer_x = ox + 1 + footer_pad as u16;
    let footer_style = Style::new().fg(palette.row_num_fg).bg(bg);
    for (i, ch) in footer.chars().enumerate() {
        let cx = footer_x + i as u16;
        if cx < ox + box_w - 1 {
            buf[(cx, fy)].set_char(ch).set_style(footer_style);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Place a string at (x, y). Does NOT fill past the text — caller should fill
/// row background separately.
fn put_text(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style) {
    use unicode_width::UnicodeWidthChar;
    let mut cx = x;
    for ch in text.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        if cx + cw > rect_right(*buf.area()) {
            break;
        }
        buf[(cx, y)].set_char(ch).set_style(style);
        cx += cw;
    }
}
