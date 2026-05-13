use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(10))
            .build();
        Self {
            base: base.trim_end_matches('/').to_string(),
            token,
            agent,
        }
    }

    fn req(&self, method: &str, path: &str) -> ureq::Request {
        let url = format!("{}{}", self.base, path);
        let mut r = self.agent.request(method, &url);
        if let Some(t) = &self.token {
            r = r.set("Authorization", &format!("Bearer {t}"));
        }
        r
    }

    pub fn search(&self, query: &str) -> Result<Vec<Track>> {
        let encoded = percent_encode(query);
        self.req("GET", &format!("/api/search?q={encoded}"))
            .call()
            .context("GET /api/search")?
            .into_json()
            .context("decode search")
    }

    pub fn list_tracks(&self) -> Result<Vec<Track>> {
        let mut out = Vec::new();
        let limit = 1000i64;
        let mut offset = 0i64;
        loop {
            let chunk: Vec<Track> = self
                .req("GET", &format!("/api/tracks?limit={limit}&offset={offset}"))
                .call()
                .context("GET /api/tracks")?
                .into_json()
                .context("decode tracks")?;
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
        self.req("GET", &format!("/api/tracks/{track_id}/tags"))
            .call()
            .context("GET tags")?
            .into_json()
            .context("decode tags")
    }

    pub fn add_user_tag(&self, track_id: i64, namespace: &str, value: &str) -> Result<AddedTag> {
        #[derive(Serialize)]
        struct Body<'a> {
            namespace: &'a str,
            value: &'a str,
        }
        self.req("POST", &format!("/api/tracks/{track_id}/tags"))
            .set("Content-Type", "application/json")
            .send_json(serde_json::to_value(Body { namespace, value })?)
            .context("POST tag")?
            .into_json()
            .context("decode added tag")
    }

    pub fn remove_user_tag(&self, track_id: i64, tag_id: i64) -> Result<()> {
        let resp = self
            .req("DELETE", &format!("/api/tracks/{track_id}/tags/{tag_id}"))
            .call()
            .context("DELETE tag")?;
        if resp.status() / 100 != 2 {
            anyhow::bail!("unexpected status {} from DELETE", resp.status());
        }
        Ok(())
    }

    pub fn list_playlists(&self) -> Result<Vec<Playlist>> {
        self.req("GET", "/api/playlists")
            .call()
            .context("GET playlists")?
            .into_json()
            .context("decode playlists")
    }

    pub fn create_playlist(&self, name: &str) -> Result<Playlist> {
        #[derive(Serialize)]
        struct Body<'a> {
            name: &'a str,
        }
        match self.req("POST", "/api/playlists").send_json(serde_json::to_value(Body { name })?) {
            Ok(r) => r.into_json().context("decode playlist"),
            Err(ureq::Error::Status(_, r)) => {
                let msg = r.into_string().unwrap_or_default();
                anyhow::bail!("server: {msg}")
            }
            Err(e) => Err(e).context("POST playlist"),
        }
    }

    pub fn rename_playlist(&self, id: i64, name: &str) -> Result<Playlist> {
        #[derive(Serialize)]
        struct Body<'a> {
            name: &'a str,
        }
        match self
            .req("PATCH", &format!("/api/playlists/{id}"))
            .send_json(serde_json::to_value(Body { name })?)
        {
            Ok(r) => r.into_json().context("decode playlist"),
            Err(ureq::Error::Status(_, r)) => {
                let msg = r.into_string().unwrap_or_default();
                anyhow::bail!("server: {msg}")
            }
            Err(e) => Err(e).context("PATCH playlist"),
        }
    }

    pub fn delete_playlist(&self, id: i64) -> Result<()> {
        let resp = self
            .req("DELETE", &format!("/api/playlists/{id}"))
            .call()
            .context("DELETE playlist")?;
        if resp.status() / 100 != 2 {
            anyhow::bail!("unexpected status {} from DELETE", resp.status());
        }
        Ok(())
    }

    pub fn get_playlist_tracks(&self, id: i64) -> Result<Vec<PlaylistTrack>> {
        self.req("GET", &format!("/api/playlists/{id}/tracks"))
            .call()
            .context("GET playlist tracks")?
            .into_json()
            .context("decode playlist tracks")
    }

    pub fn add_to_playlist(&self, playlist_id: i64, track_id: i64) -> Result<()> {
        #[derive(Serialize)]
        struct Body {
            track_id: i64,
        }
        let resp = self
            .req("POST", &format!("/api/playlists/{playlist_id}/tracks"))
            .send_json(serde_json::to_value(Body { track_id })?);
        match resp {
            Ok(r) if r.status() / 100 == 2 => Ok(()),
            Ok(r) => anyhow::bail!("unexpected status {}", r.status()),
            Err(ureq::Error::Status(_, r)) => {
                let msg = r.into_string().unwrap_or_default();
                anyhow::bail!("server: {msg}")
            }
            Err(e) => Err(e).context("POST playlist track"),
        }
    }

    pub fn set_playlist_tracks(&self, playlist_id: i64, track_ids: &[i64]) -> Result<()> {
        #[derive(Serialize)]
        struct Body<'a> {
            track_ids: &'a [i64],
        }
        let resp = self
            .req("PUT", &format!("/api/playlists/{playlist_id}/tracks"))
            .send_json(serde_json::to_value(Body { track_ids })?);
        match resp {
            Ok(r) if r.status() / 100 == 2 => Ok(()),
            Ok(r) => anyhow::bail!("unexpected status {}", r.status()),
            Err(ureq::Error::Status(_, r)) => {
                let msg = r.into_string().unwrap_or_default();
                anyhow::bail!("server: {msg}")
            }
            Err(e) => Err(e).context("PUT playlist tracks"),
        }
    }

    pub fn remove_from_playlist(&self, playlist_id: i64, track_id: i64) -> Result<()> {
        let resp = self
            .req(
                "DELETE",
                &format!("/api/playlists/{playlist_id}/tracks/{track_id}"),
            )
            .call()
            .context("DELETE playlist track")?;
        if resp.status() / 100 != 2 {
            anyhow::bail!("unexpected status {} from DELETE", resp.status());
        }
        Ok(())
    }
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

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct AddedTag {
    pub tag_id: i64,
    pub namespace: String,
    pub value: String,
}
