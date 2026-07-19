use ratatui::style::Color;

pub struct ColorPalette {
    pub column_colors: Vec<Color>,
    pub row_bg_even: Color,
    pub row_bg_odd: Color,
    pub header_bg: Color,
    pub header_fg: Color,
    pub cursor_bg: Color,
    pub cursor_fg: Color,
    pub row_num_fg: Color,
    pub bg: Color,
}

impl Default for ColorPalette {
    fn default() -> Self {
        Self {
            column_colors: vec![
                Color::Rgb(0xff, 0x6b, 0x6b), // coral red
                Color::Rgb(0x51, 0xcf, 0x66), // green
                Color::Rgb(0x33, 0x9a, 0xf0), // blue
                Color::Rgb(0xfc, 0xc4, 0x19), // gold
                Color::Rgb(0xcc, 0x5d, 0xe8), // purple
                Color::Rgb(0x22, 0xb8, 0xcf), // cyan
                Color::Rgb(0xff, 0x92, 0x2b), // orange
                Color::Rgb(0xf0, 0x65, 0x95), // pink
            ],
            row_bg_even: Color::Rgb(0x1a, 0x1a, 0x2e),
            row_bg_odd: Color::Rgb(0x22, 0x22, 0x40),
            header_bg: Color::Rgb(0x16, 0x21, 0x3e),
            header_fg: Color::Rgb(0xe2, 0xe2, 0xe2),
            cursor_bg: Color::Rgb(0x3d, 0x4a, 0x5e),
            cursor_fg: Color::White,
            row_num_fg: Color::Rgb(0x66, 0x66, 0x88),
            bg: Color::Rgb(0x1a, 0x1a, 0x2e),
        }
    }
}

impl ColorPalette {
    pub fn column_color(&self, col_idx: usize) -> Color {
        self.column_colors[col_idx % self.column_colors.len()]
    }
}
