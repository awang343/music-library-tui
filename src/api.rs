use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use ureq::http::Response;
use ureq::typestate::{WithBody, WithoutBody};
use ureq::{Body, RequestBuilder};

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Track {
    pub id: i64,
    pub path: String,
    pub title: Option<String>,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub duration_ms: Option<i64>,
    pub year: Option<i64>,
    #[serde(default)]
    pub added_at: i64,
}

impl Track {
    pub fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or_else(|| {
            self.path
                .rsplit('/')
                .next()
                .unwrap_or(&self.path)
        })
    }
    pub fn display_artist(&self) -> &str {
        self.artist
            .as_deref()
            .or(self.album_artist.as_deref())
            .unwrap_or("—")
    }
    pub fn display_album(&self) -> &str {
        self.album.as_deref().unwrap_or("—")
    }
}

#[derive(Clone)]
pub struct Client {
    base: String,
    token: Option<String>,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(base: String, token: Option<String>) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(10)))
            .http_status_as_error(false)
            .build();
        let agent: ureq::Agent = config.into();
        Self {
            base: base.trim_end_matches('/').to_string(),
            token,
            agent,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn auth_header(&self) -> Option<String> {
        self.token.as_ref().map(|t| format!("Bearer {t}"))
    }

    fn get(&self, path: &str) -> RequestBuilder<WithoutBody> {
        let mut r = self.agent.get(self.url(path));
        if let Some(a) = self.auth_header() {
            r = r.header("Authorization", a);
        }
        r
    }

    fn delete(&self, path: &str) -> RequestBuilder<WithoutBody> {
        let mut r = self.agent.delete(self.url(path));
        if let Some(a) = self.auth_header() {
            r = r.header("Authorization", a);
        }
        r
    }

    fn post(&self, path: &str) -> RequestBuilder<WithBody> {
        let mut r = self.agent.post(self.url(path));
        if let Some(a) = self.auth_header() {
            r = r.header("Authorization", a);
        }
        r
    }

    fn put(&self, path: &str) -> RequestBuilder<WithBody> {
        let mut r = self.agent.put(self.url(path));
        if let Some(a) = self.auth_header() {
            r = r.header("Authorization", a);
        }
        r
    }

    fn patch(&self, path: &str) -> RequestBuilder<WithBody> {
        let mut r = self.agent.patch(self.url(path));
        if let Some(a) = self.auth_header() {
            r = r.header("Authorization", a);
        }
        r
    }

    pub fn search(&self, query: &str) -> Result<Vec<Track>> {
        let encoded = percent_encode(query);
        let resp = self
            .get(&format!("/api/search?q={encoded}"))
            .call()
            .context("GET /api/search")?;
        decode_json(resp, "decode search")
    }

    pub fn list_tracks(&self) -> Result<Vec<Track>> {
        let mut out = Vec::new();
        let limit = 1000i64;
        let mut offset = 0i64;
        loop {
            let resp = self
                .get(&format!("/api/tracks?limit={limit}&offset={offset}"))
                .call()
                .context("GET /api/tracks")?;
            let chunk: Vec<Track> = decode_json(resp, "decode tracks")?;
            let n = chunk.len() as i64;
            out.extend(chunk);
            if n < limit {
                break;
            }
            offset += n;
        }
        Ok(out)
    }

    pub fn stream_url(&self, track_id: i64) -> String {
        format!("{}/api/tracks/{}/stream", self.base, track_id)
    }

    pub fn list_track_tags(&self, track_id: i64) -> Result<Vec<TrackTag>> {
        let resp = self
            .get(&format!("/api/tracks/{track_id}/tags"))
            .call()
            .context("GET tags")?;
        decode_json(resp, "decode tags")
    }

    pub fn add_user_tag(&self, track_id: i64, namespace: &str, value: &str) -> Result<AddedTag> {
        #[derive(Serialize)]
        struct Body<'a> {
            namespace: &'a str,
            value: &'a str,
        }
        let resp = self
            .post(&format!("/api/tracks/{track_id}/tags"))
            .send_json(Body { namespace, value })
            .context("POST tag")?;
        decode_json(resp, "decode added tag")
    }

    pub fn remove_user_tag(&self, track_id: i64, tag_id: i64) -> Result<()> {
        let resp = self
            .delete(&format!("/api/tracks/{track_id}/tags/{tag_id}"))
            .call()
            .context("DELETE tag")?;
        ensure_ok(resp)
    }

    pub fn list_playlists(&self) -> Result<Vec<Playlist>> {
        let resp = self.get("/api/playlists").call().context("GET playlists")?;
        decode_json(resp, "decode playlists")
    }

    pub fn create_playlist(&self, name: &str) -> Result<Playlist> {
        #[derive(Serialize)]
        struct Body<'a> {
            name: &'a str,
        }
        let resp = self
            .post("/api/playlists")
            .send_json(Body { name })
            .context("POST playlist")?;
        decode_json(resp, "decode playlist")
    }

    pub fn rename_playlist(&self, id: i64, name: &str) -> Result<Playlist> {
        #[derive(Serialize)]
        struct Body<'a> {
            name: &'a str,
        }
        let resp = self
            .patch(&format!("/api/playlists/{id}"))
            .send_json(Body { name })
            .context("PATCH playlist")?;
        decode_json(resp, "decode playlist")
    }

    pub fn delete_playlist(&self, id: i64) -> Result<()> {
        let resp = self
            .delete(&format!("/api/playlists/{id}"))
            .call()
            .context("DELETE playlist")?;
        ensure_ok(resp)
    }

    pub fn get_playlist_tracks(&self, id: i64) -> Result<Vec<PlaylistTrack>> {
        let resp = self
            .get(&format!("/api/playlists/{id}/tracks"))
            .call()
            .context("GET playlist tracks")?;
        decode_json(resp, "decode playlist tracks")
    }

    pub fn add_to_playlist(&self, playlist_id: i64, track_id: i64) -> Result<()> {
        #[derive(Serialize)]
        struct Body {
            track_id: i64,
        }
        let resp = self
            .post(&format!("/api/playlists/{playlist_id}/tracks"))
            .send_json(Body { track_id })
            .context("POST playlist track")?;
        ensure_ok(resp)
    }

    pub fn set_playlist_tracks(&self, playlist_id: i64, track_ids: &[i64]) -> Result<()> {
        #[derive(Serialize)]
        struct Body<'a> {
            track_ids: &'a [i64],
        }
        let resp = self
            .put(&format!("/api/playlists/{playlist_id}/tracks"))
            .send_json(Body { track_ids })
            .context("PUT playlist tracks")?;
        ensure_ok(resp)
    }

    pub fn remove_from_playlist(&self, playlist_id: i64, track_id: i64) -> Result<()> {
        let resp = self
            .delete(&format!("/api/playlists/{playlist_id}/tracks/{track_id}"))
            .call()
            .context("DELETE playlist track")?;
        ensure_ok(resp)
    }

    pub fn trigger_scan(&self) -> Result<ScanState> {
        let mut resp = self
            .post("/api/scans")
            .send_empty()
            .context("POST /api/scans")?;
        let status = resp.status();
        if status.as_u16() == 409 {
            // Already running — return current state if parseable, else generic.
            return resp
                .body_mut()
                .read_json::<ScanState>()
                .map_err(|_| anyhow::anyhow!("already running"));
        }
        if !status.is_success() {
            let msg = resp.body_mut().read_to_string().unwrap_or_default();
            anyhow::bail!("server: {msg}");
        }
        resp.body_mut().read_json().context("decode scan state")
    }

    pub fn scan_status(&self) -> Result<ScanState> {
        let resp = self.get("/api/scans").call().context("GET /api/scans")?;
        decode_json(resp, "decode scan state")
    }
}

fn decode_json<T: serde::de::DeserializeOwned>(
    mut resp: Response<Body>,
    ctx: &'static str,
) -> Result<T> {
    let status = resp.status();
    if !status.is_success() {
        let msg = resp.body_mut().read_to_string().unwrap_or_default();
        anyhow::bail!("status {}: {msg}", status.as_u16());
    }
    resp.body_mut().read_json::<T>().context(ctx)
}

fn ensure_ok(mut resp: Response<Body>) -> Result<()> {
    let status = resp.status();
    if !status.is_success() {
        let msg = resp.body_mut().read_to_string().unwrap_or_default();
        anyhow::bail!("status {}: {msg}", status.as_u16());
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ScanState {
    pub running: bool,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub last_stats: Option<ScanStats>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScanStats {
    pub seen: u64,
    pub inserted: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub failed: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub track_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct PlaylistTrack {
    pub track_id: i64,
    pub position: i64,
    pub added_at: i64,
    pub title: Option<String>,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub duration_ms: Option<i64>,
}

impl PlaylistTrack {
    pub fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or("(untitled)")
    }
    pub fn display_artist(&self) -> &str {
        self.artist
            .as_deref()
            .or(self.album_artist.as_deref())
            .unwrap_or("—")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TrackTag {
    pub tag_id: i64,
    pub namespace: String,
    pub value: String,
    pub source: String,
}

impl TrackTag {
    pub fn display(&self) -> String {
        if self.namespace.is_empty() {
            format!(":{}", self.value)
        } else {
            format!("{}:{}", self.namespace, self.value)
        }
    }
    pub fn is_user(&self) -> bool {
        self.source == "user"
    }
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct AddedTag {
    pub tag_id: i64,
    pub namespace: String,
    pub value: String,
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
