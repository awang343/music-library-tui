use crate::api::{self, Client, Playlist, PlaylistTrack, Track, TrackTag};
use crate::mpv::Mpv;
use crate::settings::Settings;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
enum Mode {
    Normal,
    Filter(String),
    TagSearch(String),
    AddTag(String),
    EditSetting(SettingsField, String),
    NewPlaylist(String),
    RenamePlaylist(i64, String),
    PickPlaylist {
        index: usize,
        track_id: i64,
    },
    SortPicker {
        index: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsField {
    ServerUrl,
    AuthToken,
}

impl SettingsField {
    const ALL: [SettingsField; 2] = [SettingsField::ServerUrl, SettingsField::AuthToken];
    fn label(&self) -> &'static str {
        match self {
            SettingsField::ServerUrl => "Server URL",
            SettingsField::AuthToken => "Auth Token",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaylistsFocus {
    List,
    Tracks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepeatMode {
    Off,
    All,
    One,
}

impl RepeatMode {
    fn cycle(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }
    fn label(self) -> &'static str {
        match self {
            RepeatMode::Off => "",
            RepeatMode::All => "↻",
            RepeatMode::One => "↻¹",
        }
    }
    fn status_label(self) -> &'static str {
        match self {
            RepeatMode::Off => "repeat off",
            RepeatMode::All => "repeat all",
            RepeatMode::One => "repeat one",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Default,
    Title,
    Artist,
    Album,
    Duration,
    Year,
    AddedAt,
}

impl SortKey {
    const ALL: [SortKey; 7] = [
        SortKey::Default,
        SortKey::Title,
        SortKey::Artist,
        SortKey::Album,
        SortKey::Duration,
        SortKey::Year,
        SortKey::AddedAt,
    ];

    fn label(self) -> &'static str {
        match self {
            SortKey::Default => "Default (artist / album / track)",
            SortKey::Title => "Title (A→Z)",
            SortKey::Artist => "Artist (A→Z)",
            SortKey::Album => "Album (A→Z)",
            SortKey::Duration => "Duration (shortest first)",
            SortKey::Year => "Year (newest first)",
            SortKey::AddedAt => "Date added (newest first)",
        }
    }

    fn short_label(self) -> &'static str {
        match self {
            SortKey::Default => "default",
            SortKey::Title => "title",
            SortKey::Artist => "artist",
            SortKey::Album => "album",
            SortKey::Duration => "duration",
            SortKey::Year => "year",
            SortKey::AddedAt => "added",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Songs,
    Playlists,
    Queue,
    Settings,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Songs, Tab::Playlists, Tab::Queue, Tab::Settings];

    fn label(&self) -> &'static str {
        match self {
            Tab::Songs => "Songs",
            Tab::Playlists => "Playlists",
            Tab::Queue => "Queue",
            Tab::Settings => "Settings",
        }
    }

    fn from_digit(c: char) -> Option<Tab> {
        match c {
            '1' => Some(Tab::Songs),
            '2' => Some(Tab::Playlists),
            '3' => Some(Tab::Queue),
            '4' => Some(Tab::Settings),
            _ => None,
        }
    }
}

pub struct App {
    client: Client,
    mpv: Mpv,
    tracks: Vec<Track>,
    filtered: Vec<usize>,
    list_state: ListState,
    tags_state: ListState,
    queue_state: ListState,
    tab: Tab,
    mode: Mode,
    settings: Settings,
    saved_settings: Settings,
    settings_field: SettingsField,
    current_tags: Vec<TrackTag>,
    playlists: Vec<Playlist>,
    playlists_state: ListState,
    playlists_focus: PlaylistsFocus,
    playlist_tracks: Vec<PlaylistTrack>,
    playlist_tracks_state: ListState,
    playlist_tracks_for: Option<i64>,
    current_tags_for: Option<i64>,
    repeat: RepeatMode,
    sort_key: SortKey,
    status_msg: String,
    show_help: bool,
    show_tags: bool,
    should_quit: bool,
    scan_polling: bool,
    last_scan_poll: Instant,
}

impl App {
    pub fn new(client: Client, mpv: Mpv, tracks: Vec<Track>, settings: Settings) -> Self {
        let filtered: Vec<usize> = (0..tracks.len()).collect();
        let mut list_state = ListState::default();
        if !filtered.is_empty() {
            list_state.select(Some(0));
        }
        let mut app = Self {
            client,
            mpv,
            tracks,
            filtered,
            list_state,
            tags_state: ListState::default(),
            queue_state: ListState::default(),
            tab: Tab::Songs,
            mode: Mode::Normal,
            saved_settings: settings.clone(),
            settings,
            settings_field: SettingsField::ServerUrl,
            current_tags: Vec::new(),
            current_tags_for: None,
            playlists: Vec::new(),
            playlists_state: ListState::default(),
            playlists_focus: PlaylistsFocus::List,
            playlist_tracks: Vec::new(),
            playlist_tracks_state: ListState::default(),
            playlist_tracks_for: None,
            repeat: RepeatMode::Off,
            sort_key: SortKey::Default,
            status_msg: String::new(),
            show_help: false,
            show_tags: false,
            should_quit: false,
            scan_polling: false,
            last_scan_poll: Instant::now(),
        };
        app.refresh_playlists();
        app
    }

    fn refresh_playlists(&mut self) {
        match self.client.list_playlists() {
            Ok(pls) => {
                let cur = self
                    .selected_playlist_id()
                    .or_else(|| pls.first().map(|p| p.id));
                self.playlists = pls;
                let new_sel = cur
                    .and_then(|id| self.playlists.iter().position(|p| p.id == id))
                    .or_else(|| if self.playlists.is_empty() { None } else { Some(0) });
                self.playlists_state.select(new_sel);
                self.refresh_playlist_tracks();
            }
            Err(e) => self.status_msg = format!("playlists: {e}"),
        }
    }

    fn selected_playlist_id(&self) -> Option<i64> {
        let i = self.playlists_state.selected()?;
        self.playlists.get(i).map(|p| p.id)
    }

    fn selected_playlist_name(&self) -> Option<String> {
        let i = self.playlists_state.selected()?;
        self.playlists.get(i).map(|p| p.name.clone())
    }

    fn refresh_playlist_tracks(&mut self) {
        let Some(id) = self.selected_playlist_id() else {
            self.playlist_tracks.clear();
            self.playlist_tracks_for = None;
            self.playlist_tracks_state.select(None);
            return;
        };
        match self.client.get_playlist_tracks(id) {
            Ok(tracks) => {
                self.playlist_tracks = tracks;
                self.playlist_tracks_for = Some(id);
                let sel = if self.playlist_tracks.is_empty() { None } else { Some(0) };
                self.playlist_tracks_state.select(sel);
            }
            Err(e) => {
                self.playlist_tracks.clear();
                self.playlist_tracks_for = Some(id);
                self.playlist_tracks_state.select(None);
                self.status_msg = format!("playlist tracks: {e}");
            }
        }
    }

    pub fn run<B>(&mut self, terminal: &mut ratatui::Terminal<B>) -> Result<()>
    where
        B: ratatui::backend::Backend,
        <B as ratatui::backend::Backend>::Error: std::error::Error + Send + Sync + 'static,
    {
        while !self.should_quit {
            terminal.draw(|f| self.render(f))?;
            if event::poll(Duration::from_millis(200))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key(key)?;
                    }
                }
            }
            self.poll_scan_if_due();
        }
        Ok(())
    }

    fn trigger_rescan(&mut self) {
        match self.client.trigger_scan() {
            Ok(state) => {
                self.status_msg = if state.running {
                    "rescanning library…".into()
                } else {
                    "scan triggered".into()
                };
                self.scan_polling = state.running;
                self.last_scan_poll = Instant::now();
            }
            Err(e) => {
                self.status_msg = format!("scan: {e}");
            }
        }
    }

    fn poll_scan_if_due(&mut self) {
        if !self.scan_polling {
            return;
        }
        if self.last_scan_poll.elapsed() < Duration::from_millis(1500) {
            return;
        }
        self.last_scan_poll = Instant::now();
        let state = match self.client.scan_status() {
            Ok(s) => s,
            Err(_) => return,
        };
        if state.running {
            return;
        }
        self.scan_polling = false;
        if let Some(err) = state.last_error {
            self.status_msg = format!("scan failed: {err}");
            return;
        }
        let summary = state.last_stats.as_ref().map(|s| {
            format!(
                "scan done: seen={} +{} ~{} ={} fail={}",
                s.seen, s.inserted, s.updated, s.unchanged, s.failed
            )
        });
        match self.client.list_tracks() {
            Ok(t) => {
                self.tracks = t;
                let prev_id = self.selected_track().map(|t| t.id);
                self.apply_filter("");
                if let Some(id) = prev_id {
                    if let Some(pos) = self.filtered.iter().position(|&i| self.tracks[i].id == id) {
                        self.list_state.select(Some(pos));
                    }
                }
                self.current_tags.clear();
                self.current_tags_for = None;
                self.status_msg = summary.unwrap_or_else(|| {
                    format!("scan done — {} tracks", self.tracks.len())
                });
            }
            Err(e) => {
                self.status_msg = format!(
                    "{} (reload failed: {e})",
                    summary.unwrap_or_else(|| "scan done".into())
                );
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        match self.mode.clone() {
            Mode::Filter(buf) => {
                self.handle_text_input(key, buf, Mode::Filter, |app, q| {
                    app.apply_filter(&q);
                });
                return Ok(());
            }
            Mode::TagSearch(mut buf) => {
                if matches!(key.code, KeyCode::Esc) {
                    self.mode = Mode::Normal;
                    self.apply_filter("");
                    return Ok(());
                }
                if matches!(key.code, KeyCode::Enter) {
                    let q = buf.clone();
                    self.mode = Mode::Normal;
                    self.run_tag_search(&q);
                    return Ok(());
                }
                match key.code {
                    KeyCode::Backspace => {
                        buf.pop();
                    }
                    KeyCode::Char(c) => {
                        buf.push(c);
                    }
                    _ => {}
                }
                self.mode = Mode::TagSearch(buf);
                return Ok(());
            }
            Mode::AddTag(buf) => {
                if matches!(key.code, KeyCode::Esc) {
                    self.mode = Mode::Normal;
                    return Ok(());
                }
                if matches!(key.code, KeyCode::Enter) {
                    self.mode = Mode::Normal;
                    self.commit_add_tag(&buf)?;
                    return Ok(());
                }
                self.edit_buf(key, buf, Mode::AddTag);
                return Ok(());
            }
            Mode::EditSetting(field, mut buf) => {
                if matches!(key.code, KeyCode::Esc) {
                    self.mode = Mode::Normal;
                    return Ok(());
                }
                if matches!(key.code, KeyCode::Enter) {
                    match field {
                        SettingsField::ServerUrl => self.settings.server_url = buf,
                        SettingsField::AuthToken => self.settings.auth_token = buf,
                    }
                    self.mode = Mode::Normal;
                    return Ok(());
                }
                match key.code {
                    KeyCode::Backspace => {
                        buf.pop();
                    }
                    KeyCode::Char(c) => {
                        buf.push(c);
                    }
                    _ => {}
                }
                self.mode = Mode::EditSetting(field, buf);
                return Ok(());
            }
            Mode::NewPlaylist(mut buf) => {
                if matches!(key.code, KeyCode::Esc) {
                    self.mode = Mode::Normal;
                    return Ok(());
                }
                if matches!(key.code, KeyCode::Enter) {
                    self.mode = Mode::Normal;
                    self.commit_new_playlist(&buf);
                    return Ok(());
                }
                match key.code {
                    KeyCode::Backspace => {
                        buf.pop();
                    }
                    KeyCode::Char(c) => {
                        buf.push(c);
                    }
                    _ => {}
                }
                self.mode = Mode::NewPlaylist(buf);
                return Ok(());
            }
            Mode::RenamePlaylist(id, mut buf) => {
                if matches!(key.code, KeyCode::Esc) {
                    self.mode = Mode::Normal;
                    return Ok(());
                }
                if matches!(key.code, KeyCode::Enter) {
                    self.mode = Mode::Normal;
                    self.commit_rename_playlist(id, &buf);
                    return Ok(());
                }
                match key.code {
                    KeyCode::Backspace => {
                        buf.pop();
                    }
                    KeyCode::Char(c) => {
                        buf.push(c);
                    }
                    _ => {}
                }
                self.mode = Mode::RenamePlaylist(id, buf);
                return Ok(());
            }
            Mode::PickPlaylist { mut index, track_id } => {
                let len = self.playlists.len();
                match key.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        return Ok(());
                    }
                    KeyCode::Enter => {
                        self.mode = Mode::Normal;
                        if let Some(p) = self.playlists.get(index) {
                            let pid = p.id;
                            let pname = p.name.clone();
                            match self.client.add_to_playlist(pid, track_id) {
                                Ok(()) => {
                                    self.status_msg = format!("added to {pname}");
                                    if self.playlist_tracks_for == Some(pid) {
                                        self.refresh_playlist_tracks();
                                    }
                                    self.refresh_playlists();
                                }
                                Err(e) => self.status_msg = format!("add failed: {e}"),
                            }
                        } else {
                            self.status_msg = "no playlists — create one first (Playlists tab)".into();
                        }
                        return Ok(());
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        if len > 0 {
                            index = (index + 1).min(len - 1);
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        index = index.saturating_sub(1);
                    }
                    _ => {}
                }
                self.mode = Mode::PickPlaylist { index, track_id };
                return Ok(());
            }
            Mode::SortPicker { mut index } => {
                let len = SortKey::ALL.len();
                match key.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        return Ok(());
                    }
                    KeyCode::Enter => {
                        let chosen = SortKey::ALL.get(index).copied();
                        self.mode = Mode::Normal;
                        if let Some(k) = chosen {
                            self.apply_sort(k);
                        }
                        return Ok(());
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        if len > 0 {
                            index = (index + 1).min(len - 1);
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        index = index.saturating_sub(1);
                    }
                    _ => {}
                }
                self.mode = Mode::SortPicker { index };
                return Ok(());
            }
            Mode::Normal => {}
        }

        // Tab switching by number key (any tab).
        if let KeyCode::Char(c) = key.code {
            if let Some(t) = Tab::from_digit(c) {
                self.tab = t;
                self.show_tags = false;
                return Ok(());
            }
        }

        // Other global keys (any tab).
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.should_quit = true;
                return Ok(());
            }
            (KeyCode::Char(' '), _) => {
                self.toggle_pause()?;
                return Ok(());
            }
            (KeyCode::Char('n'), _) => {
                self.mpv.next()?;
                return Ok(());
            }
            (KeyCode::Char('p'), _) => {
                self.mpv.prev()?;
                return Ok(());
            }
            (KeyCode::Char('?'), _) => {
                self.show_help = !self.show_help;
                return Ok(());
            }
            (KeyCode::Esc, _) if self.show_help => {
                self.show_help = false;
                return Ok(());
            }
            (KeyCode::Esc, _) if self.show_tags => {
                self.show_tags = false;
                return Ok(());
            }
            (KeyCode::Char('S'), _) => {
                self.mpv.shuffle()?;
                self.status_msg = "shuffled queue".into();
                return Ok(());
            }
            (KeyCode::Char('R'), _) => {
                self.repeat = self.repeat.cycle();
                let (lp, lf) = match self.repeat {
                    RepeatMode::Off => ("no", "no"),
                    RepeatMode::All => ("inf", "no"),
                    RepeatMode::One => ("no", "inf"),
                };
                self.mpv.set_loop_playlist(lp)?;
                self.mpv.set_loop_file(lf)?;
                self.status_msg = self.repeat.status_label().into();
                return Ok(());
            }
            (KeyCode::Char('F'), _) => {
                self.trigger_rescan();
                return Ok(());
            }
            _ => {}
        }

        match self.tab {
            Tab::Songs => self.handle_songs_key(key),
            Tab::Queue => self.handle_queue_key(key),
            Tab::Settings => self.handle_settings_key(key),
            Tab::Playlists => self.handle_playlists_key(key),
        }
    }

    fn handle_playlists_key(&mut self, key: KeyEvent) -> Result<()> {
        if matches!(key.code, KeyCode::Tab) {
            self.playlists_focus = match self.playlists_focus {
                PlaylistsFocus::List => PlaylistsFocus::Tracks,
                PlaylistsFocus::Tracks => PlaylistsFocus::List,
            };
            return Ok(());
        }
        if matches!(key.code, KeyCode::Esc) && self.playlists_focus == PlaylistsFocus::Tracks {
            self.playlists_focus = PlaylistsFocus::List;
            return Ok(());
        }
        match self.playlists_focus {
            PlaylistsFocus::List => self.handle_playlists_list_key(key),
            PlaylistsFocus::Tracks => self.handle_playlists_tracks_key(key),
        }
    }

    fn handle_playlists_list_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_playlist_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_playlist_selection(-1),
            KeyCode::Char('g') | KeyCode::Home => {
                if !self.playlists.is_empty() {
                    self.playlists_state.select(Some(0));
                    self.refresh_playlist_tracks();
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                if !self.playlists.is_empty() {
                    self.playlists_state.select(Some(self.playlists.len() - 1));
                    self.refresh_playlist_tracks();
                }
            }
            KeyCode::Enter => {
                self.refresh_playlist_tracks();
                self.playlists_focus = PlaylistsFocus::Tracks;
            }
            KeyCode::Char('N') => self.mode = Mode::NewPlaylist(String::new()),
            KeyCode::Char('r') => {
                if let (Some(id), Some(name)) =
                    (self.selected_playlist_id(), self.selected_playlist_name())
                {
                    self.mode = Mode::RenamePlaylist(id, name);
                }
            }
            KeyCode::Char('D') => self.delete_selected_playlist()?,
            KeyCode::Char('P') => self.play_selected_playlist(0)?,
            _ => {}
        }
        Ok(())
    }

    fn handle_playlists_tracks_key(&mut self, key: KeyEvent) -> Result<()> {
        let len = self.playlist_tracks.len();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if len > 0 {
                    let cur = self.playlist_tracks_state.selected().unwrap_or(0);
                    self.playlist_tracks_state.select(Some((cur + 1).min(len - 1)));
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if len > 0 {
                    let cur = self.playlist_tracks_state.selected().unwrap_or(0);
                    self.playlist_tracks_state
                        .select(Some(cur.saturating_sub(1)));
                }
            }
            KeyCode::Char('g') | KeyCode::Home => {
                if len > 0 {
                    self.playlist_tracks_state.select(Some(0));
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                if len > 0 {
                    self.playlist_tracks_state.select(Some(len - 1));
                }
            }
            KeyCode::Enter => {
                if let Some(idx) = self.playlist_tracks_state.selected() {
                    self.play_selected_playlist(idx)?;
                }
            }
            KeyCode::Char('a') => {
                if let Some(idx) = self.playlist_tracks_state.selected() {
                    if let Some(pt) = self.playlist_tracks.get(idx) {
                        let url = self.client.stream_url(pt.track_id);
                        self.mpv.enqueue(&url)?;
                        self.status_msg = format!(
                            "queued: {} — {}",
                            pt.display_artist(),
                            pt.display_title()
                        );
                    }
                }
            }
            KeyCode::Char('d') => self.remove_selected_playlist_track()?,
            KeyCode::Char('J') => self.move_playlist_track(1)?,
            KeyCode::Char('K') => self.move_playlist_track(-1)?,
            _ => {}
        }
        Ok(())
    }

    fn move_playlist_track(&mut self, delta: i32) -> Result<()> {
        let Some(pid) = self.selected_playlist_id() else {
            return Ok(());
        };
        let Some(idx) = self.playlist_tracks_state.selected() else {
            return Ok(());
        };
        let len = self.playlist_tracks.len() as i32;
        let new_idx = idx as i32 + delta;
        if new_idx < 0 || new_idx >= len {
            return Ok(());
        }
        let new_idx = new_idx as usize;

        self.playlist_tracks.swap(idx, new_idx);
        for (i, pt) in self.playlist_tracks.iter_mut().enumerate() {
            pt.position = i as i64;
        }
        self.playlist_tracks_state.select(Some(new_idx));

        let track_ids: Vec<i64> = self.playlist_tracks.iter().map(|p| p.track_id).collect();
        if let Err(e) = self.client.set_playlist_tracks(pid, &track_ids) {
            self.status_msg = format!("reorder failed: {e}");
            self.refresh_playlist_tracks();
        }
        Ok(())
    }

    fn move_playlist_selection(&mut self, delta: i32) {
        if self.playlists.is_empty() {
            return;
        }
        let cur = self.playlists_state.selected().unwrap_or(0) as i32;
        let len = self.playlists.len() as i32;
        let next = (cur + delta).clamp(0, len - 1);
        if next as usize != self.playlists_state.selected().unwrap_or(usize::MAX) {
            self.playlists_state.select(Some(next as usize));
            self.refresh_playlist_tracks();
        }
    }

    fn commit_new_playlist(&mut self, raw: &str) {
        let name = raw.trim();
        if name.is_empty() {
            self.status_msg = "playlist name is empty".into();
            return;
        }
        match self.client.create_playlist(name) {
            Ok(p) => {
                self.status_msg = format!("created playlist {}", p.name);
                self.refresh_playlists();
                if let Some(i) = self.playlists.iter().position(|x| x.id == p.id) {
                    self.playlists_state.select(Some(i));
                    self.refresh_playlist_tracks();
                }
            }
            Err(e) => self.status_msg = format!("create failed: {e}"),
        }
    }

    fn commit_rename_playlist(&mut self, id: i64, raw: &str) {
        let name = raw.trim();
        if name.is_empty() {
            self.status_msg = "name is empty".into();
            return;
        }
        match self.client.rename_playlist(id, name) {
            Ok(_) => {
                self.status_msg = "renamed".into();
                self.refresh_playlists();
            }
            Err(e) => self.status_msg = format!("rename failed: {e}"),
        }
    }

    fn delete_selected_playlist(&mut self) -> Result<()> {
        let Some(id) = self.selected_playlist_id() else {
            return Ok(());
        };
        let name = self.selected_playlist_name().unwrap_or_default();
        match self.client.delete_playlist(id) {
            Ok(()) => {
                self.status_msg = format!("deleted {name}");
                self.playlists_state.select(None);
                self.playlist_tracks_for = None;
                self.refresh_playlists();
            }
            Err(e) => self.status_msg = format!("delete failed: {e}"),
        }
        Ok(())
    }

    fn play_selected_playlist(&mut self, start_index: usize) -> Result<()> {
        let Some(_) = self.selected_playlist_id() else {
            return Ok(());
        };
        if self.playlist_tracks.is_empty() {
            self.status_msg = "playlist is empty".into();
            return Ok(());
        }
        let start = start_index.min(self.playlist_tracks.len().saturating_sub(1));
        let first = &self.playlist_tracks[start];
        let url = self.client.stream_url(first.track_id);
        self.mpv.load(&url)?;
        self.mpv.set_pause(false)?;
        for pt in &self.playlist_tracks[start + 1..] {
            let u = self.client.stream_url(pt.track_id);
            self.mpv.enqueue(&u)?;
        }
        let name = self.selected_playlist_name().unwrap_or_default();
        self.status_msg = format!(
            "playing {name} from #{} ({} tracks)",
            start + 1,
            self.playlist_tracks.len() - start
        );
        Ok(())
    }

    fn remove_selected_playlist_track(&mut self) -> Result<()> {
        let Some(pid) = self.selected_playlist_id() else {
            return Ok(());
        };
        let Some(idx) = self.playlist_tracks_state.selected() else {
            return Ok(());
        };
        let Some(pt) = self.playlist_tracks.get(idx).cloned() else {
            return Ok(());
        };
        match self.client.remove_from_playlist(pid, pt.track_id) {
            Ok(()) => {
                self.status_msg = "removed from playlist".into();
                self.refresh_playlist_tracks();
                if let Some(sel) = self.playlist_tracks_state.selected() {
                    if sel >= self.playlist_tracks.len() {
                        let new = self.playlist_tracks.len().checked_sub(1);
                        self.playlist_tracks_state.select(new);
                    }
                }
                self.refresh_playlists();
            }
            Err(e) => self.status_msg = format!("remove failed: {e}"),
        }
        Ok(())
    }

    fn handle_settings_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.settings_field = SettingsField::AuthToken;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.settings_field = SettingsField::ServerUrl;
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                let buf = match self.settings_field {
                    SettingsField::ServerUrl => self.settings.server_url.clone(),
                    SettingsField::AuthToken => self.settings.auth_token.clone(),
                };
                self.mode = Mode::EditSetting(self.settings_field, buf);
            }
            KeyCode::Char('s') => self.save_and_apply_settings()?,
            KeyCode::Char('r') | KeyCode::Esc => {
                self.settings = self.saved_settings.clone();
                self.status_msg = "settings reverted".into();
            }
            _ => {}
        }
        Ok(())
    }

    fn save_and_apply_settings(&mut self) -> Result<()> {
        if let Err(e) = self.settings.save() {
            self.status_msg = format!("save failed: {e}");
            return Ok(());
        }
        self.saved_settings = self.settings.clone();

        let token_opt = if self.settings.auth_token.is_empty() {
            None
        } else {
            Some(self.settings.auth_token.clone())
        };
        self.client = api::Client::new(self.settings.server_url.clone(), token_opt.clone());

        let mut headers = Vec::new();
        if let Some(t) = &token_opt {
            headers.push(format!("Authorization: Bearer {t}"));
        }
        let _ = self.mpv.set_http_headers(&headers);

        match self.client.list_tracks() {
            Ok(t) => {
                self.tracks = t;
                self.filtered = (0..self.tracks.len()).collect();
                self.sort_filtered();
                self.list_state.select(if self.filtered.is_empty() {
                    None
                } else {
                    Some(0)
                });
                self.current_tags.clear();
                self.current_tags_for = None;
                self.status_msg = format!("saved & reloaded ({} tracks)", self.tracks.len());
            }
            Err(e) => {
                self.tracks.clear();
                self.filtered.clear();
                self.list_state.select(None);
                self.current_tags.clear();
                self.current_tags_for = None;
                self.status_msg = format!("saved, but connect failed: {e}");
            }
        }
        Ok(())
    }

    fn handle_songs_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.show_tags {
            return self.handle_tags_key(key);
        }
        if matches!(key.code, KeyCode::Esc) {
            self.apply_filter("");
            return Ok(());
        }
        self.handle_tracks_key(key)
    }

    fn handle_tracks_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::Char('g') | KeyCode::Home => {
                if !self.filtered.is_empty() {
                    self.list_state.select(Some(0));
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                if !self.filtered.is_empty() {
                    self.list_state.select(Some(self.filtered.len() - 1));
                }
            }
            KeyCode::Enter => self.play_selected()?,
            KeyCode::Char('a') => self.enqueue_selected()?,
            KeyCode::Char('E') => self.enqueue_all_filtered()?,
            KeyCode::Char('A') => {
                if let Some(t) = self.selected_track().map(|t| t.id) {
                    if self.playlists.is_empty() {
                        self.status_msg = "no playlists — create one in the Playlists tab".into();
                    } else {
                        self.mode = Mode::PickPlaylist { index: 0, track_id: t };
                    }
                }
            }
            KeyCode::Char('/') => self.mode = Mode::Filter(String::new()),
            KeyCode::Char('T') => self.mode = Mode::TagSearch(String::new()),
            KeyCode::Char('t') => self.open_tags_popup(),
            KeyCode::Char('s') => {
                let index = SortKey::ALL
                    .iter()
                    .position(|k| *k == self.sort_key)
                    .unwrap_or(0);
                self.mode = Mode::SortPicker { index };
            }
            _ => {}
        }
        Ok(())
    }

    fn open_tags_popup(&mut self) {
        if self.selected_track().is_none() {
            self.status_msg = "no track selected".into();
            return;
        }
        self.refresh_tags();
        self.show_tags = true;
    }

    fn run_tag_search(&mut self, query: &str) {
        let q = query.trim();
        if q.is_empty() {
            self.apply_filter("");
            return;
        }
        match self.client.search(q) {
            Ok(hits) => {
                let by_id: std::collections::HashMap<i64, usize> = self
                    .tracks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| (t.id, i))
                    .collect();
                self.filtered = hits.iter().filter_map(|t| by_id.get(&t.id).copied()).collect();
                self.sort_filtered();
                self.list_state
                    .select(if self.filtered.is_empty() { None } else { Some(0) });
                self.status_msg = format!("tag search '{q}': {} hits", self.filtered.len());
            }
            Err(e) => self.status_msg = format!("search failed: {e}"),
        }
    }

    fn handle_queue_key(&mut self, key: KeyEvent) -> Result<()> {
        let len = self.mpv.snapshot().playlist.len();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if len > 0 {
                    let cur = self.queue_state.selected().unwrap_or(0);
                    self.queue_state.select(Some((cur + 1).min(len - 1)));
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if len > 0 {
                    let cur = self.queue_state.selected().unwrap_or(0);
                    self.queue_state.select(Some(cur.saturating_sub(1)));
                }
            }
            KeyCode::Char('g') | KeyCode::Home => {
                if len > 0 {
                    self.queue_state.select(Some(0));
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                if len > 0 {
                    self.queue_state.select(Some(len - 1));
                }
            }
            KeyCode::Enter => {
                if let Some(idx) = self.queue_state.selected() {
                    self.mpv.playlist_play_index(idx as i64)?;
                }
            }
            KeyCode::Char('d') => {
                if let Some(idx) = self.queue_state.selected() {
                    self.mpv.playlist_remove_index(idx as i64)?;
                }
            }
            KeyCode::Char('J') => self.move_queue_item(1)?,
            KeyCode::Char('K') => self.move_queue_item(-1)?,
            _ => {}
        }
        Ok(())
    }

    fn move_queue_item(&mut self, delta: i32) -> Result<()> {
        let len = self.mpv.snapshot().playlist.len() as i32;
        if len < 2 {
            return Ok(());
        }
        let Some(cur) = self.queue_state.selected() else {
            return Ok(());
        };
        let cur = cur as i32;
        let new = cur + delta;
        if new < 0 || new >= len {
            return Ok(());
        }
        // mpv playlist-move quirk: when src < dst, the moved entry ends up at dst-1.
        // So bump dst by one for downward moves to land exactly at `new`.
        let dst = if delta > 0 { new + 1 } else { new };
        self.mpv.playlist_move(cur as i64, dst as i64)?;
        self.queue_state.select(Some(new as usize));
        Ok(())
    }

    fn handle_tags_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_tag_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_tag_selection(-1),
            KeyCode::Char('g') | KeyCode::Home => {
                if !self.current_tags.is_empty() {
                    self.tags_state.select(Some(0));
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                if !self.current_tags.is_empty() {
                    self.tags_state.select(Some(self.current_tags.len() - 1));
                }
            }
            KeyCode::Char('a') => self.mode = Mode::AddTag(String::new()),
            KeyCode::Char('d') => self.delete_selected_tag()?,
            KeyCode::Char('t') => self.show_tags = false,
            _ => {}
        }
        Ok(())
    }

    fn move_tag_selection(&mut self, delta: i32) {
        if self.current_tags.is_empty() {
            return;
        }
        let cur = self.tags_state.selected().unwrap_or(0) as i32;
        let len = self.current_tags.len() as i32;
        let next = (cur + delta).clamp(0, len - 1);
        self.tags_state.select(Some(next as usize));
    }

    fn delete_selected_tag(&mut self) -> Result<()> {
        let Some(idx) = self.tags_state.selected() else {
            return Ok(());
        };
        let Some(tag) = self.current_tags.get(idx).cloned() else {
            return Ok(());
        };
        if !tag.is_user() {
            self.status_msg = format!("can't remove non-user tag {}", tag.display());
            return Ok(());
        }
        let Some(track_id) = self.selected_track().map(|t| t.id) else {
            return Ok(());
        };
        match self.client.remove_user_tag(track_id, tag.tag_id) {
            Ok(()) => {
                self.status_msg = format!("removed {}", tag.display());
                self.current_tags_for = None;
                self.refresh_tags();
                if self.tags_state.selected().unwrap_or(0) >= self.current_tags.len() {
                    let new = self.current_tags.len().checked_sub(1);
                    self.tags_state.select(new);
                }
            }
            Err(e) => self.status_msg = format!("remove tag failed: {e}"),
        }
        Ok(())
    }

    fn handle_text_input<F1, F2>(
        &mut self,
        key: KeyEvent,
        mut buf: String,
        wrap: F1,
        mut on_change: F2,
    ) where
        F1: Fn(String) -> Mode,
        F2: FnMut(&mut Self, String),
    {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                on_change(self, String::new());
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
            }
            KeyCode::Backspace => {
                buf.pop();
                let q = buf.clone();
                self.mode = wrap(buf);
                on_change(self, q);
            }
            KeyCode::Char(c) => {
                buf.push(c);
                let q = buf.clone();
                self.mode = wrap(buf);
                on_change(self, q);
            }
            _ => {
                self.mode = wrap(buf);
            }
        }
    }

    fn edit_buf<F: Fn(String) -> Mode>(&mut self, key: KeyEvent, mut buf: String, wrap: F) {
        match key.code {
            KeyCode::Backspace => {
                buf.pop();
                self.mode = wrap(buf);
            }
            KeyCode::Char(c) => {
                buf.push(c);
                self.mode = wrap(buf);
            }
            _ => self.mode = wrap(buf),
        }
    }

    fn apply_filter(&mut self, q: &str) {
        let needle = q.to_lowercase();
        if needle.is_empty() {
            self.filtered = (0..self.tracks.len()).collect();
        } else {
            self.filtered = self
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    let hay = format!(
                        "{} {} {}",
                        t.display_artist(),
                        t.display_title(),
                        t.display_album()
                    )
                    .to_lowercase();
                    hay.contains(&needle)
                })
                .map(|(i, _)| i)
                .collect();
        }
        self.sort_filtered();
        let sel = if self.filtered.is_empty() { None } else { Some(0) };
        self.list_state.select(sel);
    }

    fn sort_filtered(&mut self) {
        let tracks = &self.tracks;
        let key = self.sort_key;
        self.filtered.sort_by(|&a, &b| cmp_tracks(&tracks[a], &tracks[b], key));
    }

    fn apply_sort(&mut self, key: SortKey) {
        let prev_id = self.selected_track().map(|t| t.id);
        self.sort_key = key;
        self.sort_filtered();
        let new_sel = match prev_id {
            Some(id) => self.filtered.iter().position(|&i| self.tracks[i].id == id),
            None => (!self.filtered.is_empty()).then_some(0),
        };
        self.list_state.select(new_sel.or_else(|| {
            if self.filtered.is_empty() { None } else { Some(0) }
        }));
        self.status_msg = format!("sort: {}", key.short_label());
    }

    fn move_selection(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0) as i32;
        let len = self.filtered.len() as i32;
        let next = (cur + delta).clamp(0, len - 1);
        if next as usize != self.list_state.selected().unwrap_or(usize::MAX) {
            self.list_state.select(Some(next as usize));
        }
    }

    fn selected_track(&self) -> Option<&Track> {
        let i = self.list_state.selected()?;
        let idx = *self.filtered.get(i)?;
        self.tracks.get(idx)
    }

    fn refresh_tags(&mut self) {
        let Some(t) = self.selected_track().map(|t| t.id) else {
            self.current_tags.clear();
            self.current_tags_for = None;
            return;
        };
        if self.current_tags_for == Some(t) {
            return;
        }
        match self.client.list_track_tags(t) {
            Ok(tags) => {
                self.current_tags = tags;
                self.current_tags_for = Some(t);
            }
            Err(e) => {
                self.status_msg = format!("tag fetch failed: {e}");
                self.current_tags.clear();
                self.current_tags_for = Some(t);
            }
        }
        self.clamp_tag_selection();
    }

    fn clamp_tag_selection(&mut self) {
        let len = self.current_tags.len();
        let new = match (self.tags_state.selected(), len) {
            (_, 0) => None,
            (None, _) => Some(0),
            (Some(i), n) if i >= n => Some(n - 1),
            (Some(i), _) => Some(i),
        };
        self.tags_state.select(new);
    }

    fn play_selected(&mut self) -> Result<()> {
        let Some(track) = self.selected_track().cloned() else {
            return Ok(());
        };
        let url = self.client.stream_url(track.id);
        self.mpv.load(&url)?;
        self.mpv.set_pause(false)?;
        Ok(())
    }

    fn enqueue_selected(&mut self) -> Result<()> {
        let Some(track) = self.selected_track().cloned() else {
            return Ok(());
        };
        let url = self.client.stream_url(track.id);
        self.mpv.enqueue(&url)?;
        self.status_msg = format!(
            "queued: {} — {}",
            track.display_artist(),
            track.display_title()
        );
        Ok(())
    }

    fn enqueue_all_filtered(&mut self) -> Result<()> {
        if self.filtered.is_empty() {
            self.status_msg = "nothing to queue".into();
            return Ok(());
        }
        let ids: Vec<i64> = self.filtered.iter().map(|&i| self.tracks[i].id).collect();
        for id in &ids {
            let url = self.client.stream_url(*id);
            self.mpv.enqueue(&url)?;
        }
        self.status_msg = format!("queued {} tracks", ids.len());
        Ok(())
    }

    fn toggle_pause(&mut self) -> Result<()> {
        let snap = self.mpv.snapshot();
        if snap.idle_active || snap.current_path.is_none() {
            return Ok(());
        }
        self.mpv.set_pause(!snap.paused)
    }

    fn commit_add_tag(&mut self, raw: &str) -> Result<()> {
        let Some((ns, val)) = parse_tag_input(raw) else {
            self.status_msg = "tag input: '<ns>:<val>' or '<val>'".into();
            return Ok(());
        };
        let Some(track) = self.selected_track().map(|t| t.id) else {
            return Ok(());
        };
        match self.client.add_user_tag(track, &ns, &val) {
            Ok(_) => {
                self.status_msg = format!("added {}", fmt_tag(&ns, &val));
                self.current_tags_for = None; // force refresh
                self.refresh_tags();
            }
            Err(e) => self.status_msg = format!("add tag failed: {e}"),
        }
        Ok(())
    }

    fn render(&mut self, f: &mut Frame) {
        let footer_height = if matches!(self.mode, Mode::Normal) { 3 } else { 4 };
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(footer_height),
            ])
            .split(f.area());

        self.render_tabs(f, outer[0]);
        match self.tab {
            Tab::Songs => self.render_songs(f, outer[1]),
            Tab::Playlists => self.render_playlists(f, outer[1]),
            Tab::Queue => self.render_queue(f, outer[1]),
            Tab::Settings => self.render_settings(f, outer[1]),
        }
        self.render_footer(f, outer[2]);

        // Modal overlays on top of everything.
        if self.tab == Tab::Songs && self.show_tags {
            self.render_tags_overlay(f);
        }
        if self.show_help {
            self.render_help_overlay(f);
        }
        if let Mode::PickPlaylist { .. } = &self.mode {
            self.render_pick_playlist_overlay(f);
        }
        if let Mode::SortPicker { .. } = &self.mode {
            self.render_sort_picker_overlay(f);
        }
    }

    fn render_sort_picker_overlay(&self, f: &mut Frame) {
        let Mode::SortPicker { index } = self.mode else {
            return;
        };
        let area = centered_rect(50, 50, f.area());
        f.render_widget(ratatui::widgets::Clear, area);

        let items: Vec<ListItem> = SortKey::ALL
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let is_cursor = i == index;
                let is_active = *k == self.sort_key;
                let prefix = if is_cursor { "> " } else { "  " };
                let marker = if is_active { " *" } else { "" };
                let style = if is_cursor {
                    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                } else if is_active {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(Span::styled(
                    format!("{prefix}{}{marker}", k.label()),
                    style,
                )))
            })
            .collect();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Sort by  (j/k, ⏎ confirm, Esc cancel) ");
        let list = List::new(items).block(block);
        f.render_widget(list, area);
    }

    fn render_help_overlay(&self, f: &mut Frame) {
        let area = centered_rect(70, 80, f.area());
        f.render_widget(ratatui::widgets::Clear, area);

        let header = |s: &'static str| {
            Line::from(Span::styled(
                s,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))
        };
        let item = |k: &str, d: &str| {
            Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{:<16}", k),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(d.to_string()),
            ])
        };

        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(header("Global"));
        lines.push(item("q / Ctrl+C", "quit"));
        lines.push(item("space", "play / pause"));
        lines.push(item("n / p", "next / previous"));
        lines.push(item("S", "shuffle queue"));
        lines.push(item("R", "cycle repeat (off/all/one)"));
        lines.push(item("F", "rescan library"));
        lines.push(item("?", "toggle this help"));
        lines.push(item("1-4", "switch tabs"));
        lines.push(Line::raw(""));

        match self.tab {
            Tab::Songs => {
                if self.show_tags {
                    lines.push(header("Songs — tags popup"));
                    lines.push(item("j/k", "navigate tags"));
                    lines.push(item("g / G", "top / bottom"));
                    lines.push(item("a", "add tag"));
                    lines.push(item("d", "remove user tag"));
                    lines.push(item("t / Esc", "close tags popup"));
                } else {
                    lines.push(header("Songs"));
                    lines.push(item("j/k, PgUp/Dn", "move selection"));
                    lines.push(item("g / G", "top / bottom"));
                    lines.push(item("⏎", "play selected"));
                    lines.push(item("a", "queue selected"));
                    lines.push(item("E", "queue all (filtered)"));
                    lines.push(item("A", "add to playlist"));
                    lines.push(item("t", "show tags for selected"));
                    lines.push(item("/", "filter"));
                    lines.push(item("T", "tag search"));
                    lines.push(item("s", "sort by…"));
                    lines.push(item("Esc", "clear filter"));
                }
            }
            Tab::Playlists => match self.playlists_focus {
                PlaylistsFocus::List => {
                    lines.push(header("Playlists"));
                    lines.push(item("j/k", "navigate"));
                    lines.push(item("⏎", "open playlist"));
                    lines.push(item("P", "play playlist"));
                    lines.push(item("N", "new playlist"));
                    lines.push(item("r", "rename"));
                    lines.push(item("D", "delete"));
                    lines.push(item("⇥", "tracks pane"));
                }
                PlaylistsFocus::Tracks => {
                    lines.push(header("Playlist tracks"));
                    lines.push(item("j/k", "navigate"));
                    lines.push(item("J / K", "reorder down / up"));
                    lines.push(item("⏎", "play from here"));
                    lines.push(item("a", "queue track"));
                    lines.push(item("d", "remove from playlist"));
                    lines.push(item("⇥ / Esc", "back to playlists"));
                }
            },
            Tab::Queue => {
                lines.push(header("Queue"));
                lines.push(item("j/k", "navigate"));
                lines.push(item("g / G", "top / bottom"));
                lines.push(item("J / K", "reorder down / up"));
                lines.push(item("⏎", "jump to track"));
                lines.push(item("d", "remove from queue"));
            }
            Tab::Settings => {
                lines.push(header("Settings"));
                lines.push(item("j/k", "navigate fields"));
                lines.push(item("⏎ / e", "edit field"));
                lines.push(item("s", "save changes"));
                lines.push(item("r / Esc", "revert"));
            }
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Help (? to close) ");
        let p = Paragraph::new(lines).block(block);
        f.render_widget(p, area);
    }

    fn render_playlists(&mut self, f: &mut Frame, area: Rect) {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(area);

        let items: Vec<ListItem> = self
            .playlists
            .iter()
            .map(|p| {
                let line = Line::from(vec![
                    Span::styled(
                        p.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  ({})", p.track_count),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ]);
                ListItem::new(line)
            })
            .collect();
        let title = format!(" Playlists ({}) ", self.playlists.len());
        let block = pane_block(title, self.playlists_focus == PlaylistsFocus::List);
        let mut list = List::new(items).block(block);
        if self.playlists_focus == PlaylistsFocus::List {
            list = list.highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        }
        f.render_stateful_widget(list, split[0], &mut self.playlists_state);

        let pl_name = self.selected_playlist_name().unwrap_or_default();
        let track_items: Vec<ListItem> = self
            .playlist_tracks
            .iter()
            .map(|pt| {
                let line = Line::from(vec![
                    Span::styled(
                        format!("{:>3}. ", pt.position + 1),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                    Span::styled(
                        pt.display_artist().to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::raw(pt.display_title().to_string()),
                ]);
                ListItem::new(line)
            })
            .collect();
        let title = if pl_name.is_empty() {
            " Tracks ".to_string()
        } else {
            format!(" {} ({}) ", pl_name, self.playlist_tracks.len())
        };
        let block = pane_block(title, self.playlists_focus == PlaylistsFocus::Tracks);
        let mut list = List::new(track_items).block(block);
        if self.playlists_focus == PlaylistsFocus::Tracks {
            list = list.highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        }
        f.render_stateful_widget(list, split[1], &mut self.playlist_tracks_state);
    }

    fn render_pick_playlist_overlay(&self, f: &mut Frame) {
        let Mode::PickPlaylist { index, .. } = self.mode else {
            return;
        };
        let area = centered_rect(60, 60, f.area());
        f.render_widget(ratatui::widgets::Clear, area);

        let items: Vec<ListItem> = self
            .playlists
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let prefix = if i == index { "> " } else { "  " };
                let style = if i == index {
                    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(Span::styled(
                    format!("{prefix}{}  ({})", p.name, p.track_count),
                    style,
                )))
            })
            .collect();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" add to playlist  (j/k, ⏎ confirm, Esc cancel) ");
        let list = List::new(items).block(block);
        f.render_widget(list, area);
    }

    fn render_songs(&mut self, f: &mut Frame, area: Rect) {
        self.render_list(f, area);
    }

    fn render_tabs(&self, f: &mut Frame, area: Rect) {
        let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
        for (i, t) in Tab::ALL.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("   "));
            }
            let is_active = self.tab == *t;
            let style = if is_active {
                Style::default()
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::DIM)
            };
            spans.push(Span::styled(
                format!(" {} {} ", i + 1, t.label()),
                style,
            ));
        }
        let p = Paragraph::new(Line::from(spans));
        f.render_widget(p, area);
    }

    fn render_settings(&self, f: &mut Frame, area: Rect) {
        let block = Block::default().borders(Borders::ALL).title(" Settings ");
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

        for field in SettingsField::ALL {
            let val: String = match field {
                SettingsField::ServerUrl => self.settings.server_url.clone(),
                SettingsField::AuthToken => self.settings.auth_token.clone(),
            };
            let is_selected = self.settings_field == field;
            let is_editing = matches!(&self.mode, Mode::EditSetting(f, _) if *f == field);

            let prefix = if is_selected { "> " } else { "  " };
            let label_style = if is_selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let val_text = if let Mode::EditSetting(f, buf) = &self.mode {
                if *f == field {
                    format!("{buf}_")
                } else {
                    val
                }
            } else {
                val
            };
            let val_style = if is_editing {
                Style::default().fg(Color::Yellow)
            } else if is_selected {
                Style::default()
            } else {
                Style::default().add_modifier(Modifier::DIM)
            };

            lines.push(Line::from(vec![
                Span::raw(prefix.to_string()),
                Span::styled(format!("{:<12}", field.label()), label_style),
                Span::raw("  "),
                Span::styled(val_text, val_style),
            ]));
            lines.push(Line::raw(""));
        }

        let dirty = self.settings.server_url != self.saved_settings.server_url
            || self.settings.auth_token != self.saved_settings.auth_token;
        if dirty {
            lines.push(Line::from(Span::styled(
                "  unsaved changes — 's' save, 'r'/Esc revert",
                Style::default().fg(Color::Yellow),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                format!("  config: {}", crate::settings::Settings::config_path().display()),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }

        let p = Paragraph::new(lines);
        f.render_widget(p, inner);
    }

    fn render_queue(&mut self, f: &mut Frame, area: Rect) {
        let snap = self.mpv.snapshot();
        let items: Vec<ListItem> = snap
            .playlist
            .iter()
            .map(|entry| {
                let id = track_id_from_url(&entry.url);
                let track = id.and_then(|i| self.tracks.iter().find(|t| t.id == i));
                let label = match track {
                    Some(t) => format!("{} — {}", t.display_artist(), t.display_title()),
                    None => entry.url.clone(),
                };
                let mark = if entry.current {
                    if snap.paused { "‖ " } else { "▶ " }
                } else {
                    "  "
                };
                let style = if entry.current {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(Span::styled(format!("{mark}{label}"), style)))
            })
            .collect();
        // Keep selection in range.
        let len = snap.playlist.len();
        match (self.queue_state.selected(), len) {
            (_, 0) => self.queue_state.select(None),
            (None, _) => self.queue_state.select(Some(0)),
            (Some(i), n) if i >= n => self.queue_state.select(Some(n - 1)),
            _ => {}
        }

        let title = format!(" Queue ({}) ", len);
        let block = Block::default().borders(Borders::ALL).title(title);
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        f.render_stateful_widget(list, area, &mut self.queue_state);
    }

    fn render_list(&mut self, f: &mut Frame, area: Rect) {
        let snap = self.mpv.snapshot();
        let now_track_id = snap.current_path.as_deref().and_then(track_id_from_url);
        let items: Vec<ListItem> = self
            .filtered
            .iter()
            .map(|&i| {
                let t = &self.tracks[i];
                let mark = if Some(t.id) == now_track_id {
                    if snap.paused { "‖ " } else { "▶ " }
                } else {
                    "  "
                };
                let line = Line::from(vec![
                    Span::raw(mark),
                    Span::styled(
                        t.display_artist().to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::raw(t.display_title().to_string()),
                    Span::raw("  "),
                    Span::styled(
                        format!("[{}]", t.display_album()),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ]);
                ListItem::new(line)
            })
            .collect();

        let title = format!(
            " muserv — {} / {} ",
            self.filtered.len(),
            self.tracks.len()
        );
        let block = pane_block(title, true);
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("");

        f.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_tags_overlay(&mut self, f: &mut Frame) {
        let area = centered_rect(60, 70, f.area());
        f.render_widget(ratatui::widgets::Clear, area);

        let items: Vec<ListItem> = self
            .current_tags
            .iter()
            .map(|t| {
                let style = if t.is_user() {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().add_modifier(Modifier::DIM)
                };
                let badge = if t.is_user() { " *" } else { "" };
                ListItem::new(Line::from(vec![Span::styled(
                    format!("{}{}", t.display(), badge),
                    style,
                )]))
            })
            .collect();

        let track_label = self
            .selected_track()
            .map(|t| format!("{} — {}", t.display_artist(), t.display_title()))
            .unwrap_or_else(|| "no track".into());
        let title = format!(" Tags — {} ({}) ", track_label, self.current_tags.len());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(title);
        if self.tags_state.selected().is_none() && !self.current_tags.is_empty() {
            self.tags_state.select(Some(0));
        }
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        f.render_stateful_widget(list, area, &mut self.tags_state);
    }

    fn render_footer(&self, f: &mut Frame, area: Rect) {
        let block = Block::default().borders(Borders::ALL);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let normal = matches!(self.mode, Mode::Normal);
        let constraints: &[Constraint] = if normal {
            &[Constraint::Length(1)]
        } else {
            &[Constraint::Length(1), Constraint::Length(1)]
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        let status_width = self.status_msg.chars().count() as u16;
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(status_width)])
            .split(rows[0]);
        f.render_widget(Paragraph::new(self.now_playing_line()), cols[0]);
        if status_width > 0 {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    self.status_msg.clone(),
                    Style::default().fg(Color::Yellow),
                )))
                .alignment(Alignment::Right),
                cols[1],
            );
        }

        if !normal {
            f.render_widget(Paragraph::new(self.prompt_or_hints_line()), rows[1]);
        }
    }

    fn now_playing_line(&self) -> Line<'static> {
        let snap = self.mpv.snapshot();
        let nothing = snap.idle_active || snap.current_path.is_none();
        if nothing {
            return Line::from(Span::styled(
                "■ stopped",
                Style::default().add_modifier(Modifier::BOLD),
            ));
        }
        let glyph = if snap.paused { "‖" } else { "▶" };
        let url = snap.current_path.as_deref().unwrap_or("");
        let track = track_id_from_url(url)
            .and_then(|id| self.tracks.iter().find(|t| t.id == id));
        let label = match track {
            Some(t) => format!("{} — {}", t.display_artist(), t.display_title()),
            None => url.to_string(),
        };
        let time = format!(
            "   {} / {}",
            snap.time_pos.map(fmt_time).unwrap_or_else(|| "—".into()),
            snap.duration.map(fmt_time).unwrap_or_else(|| "—".into()),
        );
        let repeat = self.repeat.label();
        let mut spans = vec![
            Span::styled(format!("{glyph} "), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(label),
            Span::styled(time, Style::default().add_modifier(Modifier::DIM)),
        ];
        if !repeat.is_empty() {
            spans.push(Span::styled(
                format!("   {repeat}"),
                Style::default().fg(Color::Cyan),
            ));
        }
        Line::from(spans)
    }

    fn prompt_or_hints_line(&self) -> Line<'static> {
        match &self.mode {
            Mode::Filter(buf) => Line::from(vec![
                Span::styled("/", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(buf.clone()),
                Span::raw("_"),
            ]),
            Mode::TagSearch(buf) => Line::from(vec![
                Span::styled("T", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(" tag: "),
                Span::raw(buf.clone()),
                Span::raw("_"),
            ]),
            Mode::AddTag(buf) => Line::from(vec![
                Span::styled("+tag ", Style::default().fg(Color::Green)),
                Span::raw(buf.clone()),
                Span::raw("_"),
            ]),
            Mode::EditSetting(field, _) => Line::from(vec![
                Span::styled(
                    format!("editing {}: ", field.label()),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    "⏎ commit  Esc cancel",
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ]),
            Mode::NewPlaylist(buf) => Line::from(vec![
                Span::styled("new playlist ", Style::default().fg(Color::Green)),
                Span::raw(buf.clone()),
                Span::raw("_"),
            ]),
            Mode::RenamePlaylist(_, buf) => Line::from(vec![
                Span::styled("rename ", Style::default().fg(Color::Yellow)),
                Span::raw(buf.clone()),
                Span::raw("_"),
            ]),
            Mode::PickPlaylist { .. } => Line::from(Span::styled(
                "pick playlist (j/k, ⏎ confirm, Esc cancel)",
                Style::default().fg(Color::Cyan),
            )),
            Mode::SortPicker { .. } => Line::from(Span::styled(
                "sort songs (j/k, ⏎ confirm, Esc cancel)",
                Style::default().fg(Color::Cyan),
            )),
            Mode::Normal => Line::raw(""),
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_y = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_y[1])[1]
}

fn cmp_tracks(a: &Track, b: &Track, key: SortKey) -> std::cmp::Ordering {
    let s = |x: &str| x.to_lowercase();
    match key {
        SortKey::Default => {
            let aa = a.album_artist.as_deref().unwrap_or("");
            let ba = b.album_artist.as_deref().unwrap_or("");
            s(aa).cmp(&s(ba))
                .then_with(|| {
                    s(a.album.as_deref().unwrap_or(""))
                        .cmp(&s(b.album.as_deref().unwrap_or("")))
                })
                .then(a.disc_no.unwrap_or(0).cmp(&b.disc_no.unwrap_or(0)))
                .then(a.track_no.unwrap_or(0).cmp(&b.track_no.unwrap_or(0)))
                .then_with(|| s(a.display_title()).cmp(&s(b.display_title())))
        }
        SortKey::Title => s(a.display_title()).cmp(&s(b.display_title())),
        SortKey::Artist => s(a.display_artist())
            .cmp(&s(b.display_artist()))
            .then_with(|| s(a.display_album()).cmp(&s(b.display_album())))
            .then(a.disc_no.unwrap_or(0).cmp(&b.disc_no.unwrap_or(0)))
            .then(a.track_no.unwrap_or(0).cmp(&b.track_no.unwrap_or(0))),
        SortKey::Album => s(a.display_album())
            .cmp(&s(b.display_album()))
            .then(a.disc_no.unwrap_or(0).cmp(&b.disc_no.unwrap_or(0)))
            .then(a.track_no.unwrap_or(0).cmp(&b.track_no.unwrap_or(0))),
        SortKey::Duration => a
            .duration_ms
            .unwrap_or(i64::MAX)
            .cmp(&b.duration_ms.unwrap_or(i64::MAX)),
        // Year and AddedAt are "newest first": descending.
        SortKey::Year => b
            .year
            .unwrap_or(i64::MIN)
            .cmp(&a.year.unwrap_or(i64::MIN))
            .then_with(|| s(a.display_album()).cmp(&s(b.display_album())))
            .then(a.disc_no.unwrap_or(0).cmp(&b.disc_no.unwrap_or(0)))
            .then(a.track_no.unwrap_or(0).cmp(&b.track_no.unwrap_or(0))),
        SortKey::AddedAt => b.added_at.cmp(&a.added_at),
    }
}

fn pane_block(title: String, focused: bool) -> Block<'static> {
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title)
}

fn track_id_from_url(url: &str) -> Option<i64> {
    let mut segs = url.split('/');
    while let Some(s) = segs.next() {
        if s == "tracks" {
            return segs.next().and_then(|n| n.parse().ok());
        }
    }
    None
}

fn parse_tag_input(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (ns, val) = match s.split_once(':') {
        Some((n, v)) => (n.trim(), v.trim()),
        None => ("", s),
    };
    if val.is_empty() {
        return None;
    }
    Some((ns.to_string(), val.to_string()))
}

fn fmt_tag(ns: &str, val: &str) -> String {
    if ns.is_empty() {
        format!(":{val}")
    } else {
        format!("{ns}:{val}")
    }
}

fn fmt_time(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let m = total / 60;
    let s = total % 60;
    if m >= 60 {
        let h = m / 60;
        let m = m % 60;
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}
