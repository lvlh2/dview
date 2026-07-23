mod app;
mod colors;
mod data;
mod ui;

use std::io;

use clap::Parser;
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::backend::CrosstermBackend;

use crate::app::App;

#[derive(Parser)]
#[command(name = "dview", version, about = "Terminal data file viewer")]
struct Cli {
    /// Path to the data file (.csv, .tsv, .xls, .xlsx, .parquet)
    file: String,
}

fn main() -> anyhow::Result<()> {
    // Install panic hook to restore terminal before printing panic info.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(info);
    }));

    let cli = Cli::parse();

    // Load the data file.
    let sheets = data::load_file(std::path::Path::new(&cli.file))?;

    // Setup terminal.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;
    terminal.clear()?;

    // Run app.
    let mut app = App::new(sheets);
    let result = app.run(&mut terminal);

    // Cleanup terminal.
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;

    result
}
