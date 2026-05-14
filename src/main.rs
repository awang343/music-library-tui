use anyhow::{Context, Result};
use clap::Parser;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;

mod api;
mod app;
mod mpv;
mod settings;

#[derive(Parser, Debug)]
#[command(name = "mutui", about = "TUI client for Muserv")]
struct Cli {
    /// Server base URL (overrides settings file).
    #[arg(short, long, env = "MUSIC_LIB_URL")]
    server: Option<String>,

    /// Bearer token (overrides settings file).
    #[arg(short, long, env = "MUSIC_LIB_TOKEN")]
    token: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut s = settings::Settings::load();
    if let Some(url) = cli.server {
        s.server_url = url;
    }
    if let Some(tok) = cli.token {
        s.auth_token = tok;
    }
    if s.server_url.is_empty() {
        s.server_url = "http://127.0.0.1:7700".into();
    }

    let token_opt = if s.auth_token.is_empty() {
        None
    } else {
        Some(s.auth_token.clone())
    };
    let client = api::Client::new(s.server_url.clone(), token_opt.clone());
    let tracks = match client.list_tracks() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("warning: could not reach {}: {e}", s.server_url);
            eprintln!("starting with empty library — use the Settings tab to fix.");
            Vec::new()
        }
    };

    let mut headers = Vec::new();
    if let Some(t) = &token_opt {
        headers.push(format!("Authorization: Bearer {t}"));
    }
    let mpv = mpv::Mpv::spawn(&headers).context("spawning mpv")?;

    let mut app = app::App::new(client, mpv, tracks, s);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = app.run(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}
