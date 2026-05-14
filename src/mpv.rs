use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, Default)]
pub struct PlayerState {
    pub time_pos: Option<f64>,
    pub duration: Option<f64>,
    pub paused: bool,
    pub idle_active: bool,
    pub current_path: Option<String>,
    pub playlist: Vec<PlaylistEntry>,
}

#[derive(Debug, Clone)]
pub struct PlaylistEntry {
    pub url: String,
    pub current: bool,
}

pub struct Mpv {
    child: Child,
    socket_path: PathBuf,
    writer: UnixStream,
    request_id: u64,
    state: Arc<Mutex<PlayerState>>,
}

impl Mpv {
    pub fn spawn(http_headers: &[String]) -> Result<Self> {
        let socket_path = std::env::temp_dir()
            .join(format!("mutui-mpv-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket_path);

        let mut cmd = Command::new("mpv");
        cmd.arg("--idle=yes")
            .arg("--no-video")
            .arg("--no-terminal")
            .arg("--really-quiet")
            .arg("--audio-display=no")
            .arg(format!("--input-ipc-server={}", socket_path.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        // Opportunistic MPRIS — load mpv-mpris if installed so playerctl works.
        for c in [
            "/usr/lib/mpv/scripts/mpris.so",
            "/usr/lib/mpv-mpris/mpris.so",
            "/usr/lib/x86_64-linux-gnu/mpv/scripts/mpris.so",
        ] {
            if std::path::Path::new(c).exists() {
                cmd.arg(format!("--script={c}"));
                break;
            }
        }

        let child = cmd.spawn().context("spawning mpv")?;

        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            if let Ok(s) = UnixStream::connect(&socket_path) {
                break s;
            }
            if Instant::now() >= deadline {
                bail!("mpv ipc socket {} did not appear", socket_path.display());
            }
            thread::sleep(Duration::from_millis(50));
        };

        let writer = stream.try_clone().context("clone mpv socket")?;
        let reader = BufReader::new(stream);
        let state = Arc::new(Mutex::new(PlayerState::default()));
        let state_for_reader = state.clone();

        thread::spawn(move || {
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if v.get("event").and_then(Value::as_str) != Some("property-change") {
                    continue;
                }
                let name = v.get("name").and_then(Value::as_str);
                let data = v.get("data");
                let Ok(mut s) = state_for_reader.lock() else { break };
                match name {
                    Some("time-pos") => s.time_pos = data.and_then(Value::as_f64),
                    Some("duration") => s.duration = data.and_then(Value::as_f64),
                    Some("pause") => {
                        s.paused = data.and_then(Value::as_bool).unwrap_or(false)
                    }
                    Some("idle-active") => {
                        s.idle_active = data.and_then(Value::as_bool).unwrap_or(false)
                    }
                    Some("path") => {
                        s.current_path = data.and_then(Value::as_str).map(str::to_owned);
                    }
                    Some("playlist") => {
                        s.playlist = data
                            .and_then(Value::as_array)
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|e| {
                                        let obj = e.as_object()?;
                                        let url = obj.get("filename")?.as_str()?.to_owned();
                                        let current = obj
                                            .get("current")
                                            .and_then(Value::as_bool)
                                            .unwrap_or(false);
                                        Some(PlaylistEntry { url, current })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                    }
                    _ => {}
                }
            }
        });

        let mut mpv = Self {
            child,
            socket_path,
            writer,
            request_id: 0,
            state,
        };

        if !http_headers.is_empty() {
            mpv.send(json!({
                "command": ["set_property", "http-header-fields", http_headers]
            }))?;
        }
        mpv.send(json!({"command": ["observe_property", 1, "time-pos"]}))?;
        mpv.send(json!({"command": ["observe_property", 2, "duration"]}))?;
        mpv.send(json!({"command": ["observe_property", 3, "pause"]}))?;
        mpv.send(json!({"command": ["observe_property", 4, "idle-active"]}))?;
        mpv.send(json!({"command": ["observe_property", 5, "path"]}))?;
        mpv.send(json!({"command": ["observe_property", 6, "playlist"]}))?;
        Ok(mpv)
    }

    fn send(&mut self, mut cmd: Value) -> Result<()> {
        self.request_id += 1;
        if let Some(obj) = cmd.as_object_mut() {
            obj.insert("request_id".into(), json!(self.request_id));
        }
        let mut line = serde_json::to_vec(&cmd)?;
        line.push(b'\n');
        self.writer
            .write_all(&line)
            .context("write to mpv socket")?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn load(&mut self, url: &str) -> Result<()> {
        self.send(json!({"command": ["loadfile", url, "replace"]}))
    }

    pub fn enqueue(&mut self, url: &str) -> Result<()> {
        self.send(json!({"command": ["loadfile", url, "append-play"]}))
    }

    pub fn playlist_remove_index(&mut self, index: i64) -> Result<()> {
        self.send(json!({"command": ["playlist-remove", index]}))
    }

    pub fn playlist_play_index(&mut self, index: i64) -> Result<()> {
        self.send(json!({"command": ["playlist-play-index", index]}))
    }

    pub fn playlist_move(&mut self, from: i64, to: i64) -> Result<()> {
        self.send(json!({"command": ["playlist-move", from, to]}))
    }

    pub fn next(&mut self) -> Result<()> {
        self.send(json!({"command": ["playlist-next"]}))
    }

    pub fn prev(&mut self) -> Result<()> {
        self.send(json!({"command": ["playlist-prev"]}))
    }

    /// Shuffle the queue while keeping the currently-playing track at index 0.
    /// If nothing is playing, falls back to mpv's full playlist-shuffle.
    pub fn shuffle(&mut self) -> Result<()> {
        let snap = self.snapshot();
        let n = snap.playlist.len();
        if n < 2 {
            return Ok(());
        }
        let Some(ci) = snap.playlist.iter().position(|e| e.current) else {
            return self.send(json!({"command": ["playlist-shuffle"]}));
        };

        let urls: Vec<String> = snap.playlist.iter().map(|e| e.url.clone()).collect();
        let mut others: Vec<String> = urls
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != ci)
            .map(|(_, u)| u.clone())
            .collect();
        shuffle_in_place(&mut others);

        let mut target: Vec<String> = Vec::with_capacity(n);
        target.push(urls[ci].clone());
        target.extend(others);

        // Apply via playlist-move from highest-to-lowest, placing target[i] at i.
        let mut working: Vec<String> = urls;
        for i in 0..n {
            if working[i] == target[i] {
                continue;
            }
            let Some(rel_j) = working.iter().skip(i).position(|u| u == &target[i]) else {
                continue;
            };
            let j = i + rel_j;
            self.send(json!({"command": ["playlist-move", j, i]}))?;
            let item = working.remove(j);
            working.insert(i, item);
        }
        Ok(())
    }

    pub fn set_loop_playlist(&mut self, value: &str) -> Result<()> {
        self.send(json!({"command": ["set_property", "loop-playlist", value]}))
    }

    pub fn set_loop_file(&mut self, value: &str) -> Result<()> {
        self.send(json!({"command": ["set_property", "loop-file", value]}))
    }

    pub fn set_pause(&mut self, paused: bool) -> Result<()> {
        self.send(json!({"command": ["set_property", "pause", paused]}))
    }

    pub fn set_http_headers(&mut self, headers: &[String]) -> Result<()> {
        self.send(json!({
            "command": ["set_property", "http-header-fields", headers]
        }))
    }

    pub fn snapshot(&self) -> PlayerState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        let _ = self.send(json!({"command": ["quit"]}));
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn shuffle_in_place<T>(v: &mut [T]) {
    let mut state = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0xdead_beef);
    if state == 0 {
        state = 0xdead_beef;
    }
    for i in (1..v.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state as usize) % (i + 1);
        v.swap(i, j);
    }
}
