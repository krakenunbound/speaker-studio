use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Segment {
    pub speaker: u8,
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Turn {
    pub speaker: u8,
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpeakerInfo {
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub status: String,
    pub kind: String,
    pub visual: String,
    pub media_file: String,
    pub duration: f64,
    pub peaks: Vec<f32>,
    #[serde(default)]
    pub party_peaks: Vec<f32>,
    #[serde(default)]
    pub mic_peaks: Vec<f32>,
    pub segments: Vec<Segment>,
    pub turns: Vec<Turn>,
    pub speakers: BTreeMap<String, SpeakerInfo>,
    pub max_speakers: u8,
    pub message: String,
}

impl Session {
    pub fn new(title: &str, kind: &str) -> Self {
        Self {
            id: new_id(),
            title: clean_title(title),
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            status: "ready".into(),
            kind: kind.into(),
            visual: "audio".into(),
            media_file: String::new(),
            duration: 0.0,
            peaks: Vec::new(),
            party_peaks: Vec::new(),
            mic_peaks: Vec::new(),
            segments: Vec::new(),
            turns: Vec::new(),
            speakers: BTreeMap::new(),
            max_speakers: 8,
            message: String::new(),
        }
    }

    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join("sessions").join(&self.id)
    }

    pub fn save(&self, root: &Path) -> io::Result<()> {
        let dir = self.dir(root);
        fs::create_dir_all(&dir)?;
        let target = dir.join("session.json");
        let temp = dir.join("session.json.tmp");
        let backup = dir.join("session.json.bak");
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        {
            let mut file = File::create(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        if target.exists() {
            let _ = fs::remove_file(&backup);
            fs::rename(&target, &backup)?;
        }
        if let Err(error) = fs::rename(&temp, &target) {
            if backup.exists() && !target.exists() {
                let _ = fs::rename(&backup, &target);
            }
            return Err(error);
        }
        Ok(())
    }
}

pub fn load_all(root: &Path) -> Vec<Session> {
    let mut sessions = Vec::new();
    let Ok(entries) = fs::read_dir(root.join("sessions")) else {
        return sessions;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(session) = load_session(&path) {
                sessions.push(session);
            }
        }
    }
    sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    sessions
}

fn load_session(dir: &Path) -> Option<Session> {
    read_session(&dir.join("session.json")).or_else(|| read_session(&dir.join("session.json.bak")))
}

fn read_session(path: &Path) -> Option<Session> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn clean_title(value: &str) -> String {
    let title = value.trim().replace(['\r', '\n'], " ");
    let title: String = title.chars().take(140).collect();
    if title.is_empty() {
        "Untitled".into()
    } else {
        title
    }
}

pub fn default_speaker_name(speaker: u8) -> String {
    match speaker {
        8 => "Me".into(),
        254 => "Overlap".into(),
        255 => "Unassigned".into(),
        _ => format!("Speaker {}", speaker + 1),
    }
}

pub fn clean_name(value: &str, speaker: u8) -> String {
    let name: String = value.trim().replace(['\r', '\n'], " ").chars().take(160).collect();
    if name.is_empty() {
        default_speaker_name(speaker)
    } else {
        name
    }
}

pub fn ensure_speakers(session: &mut Session) {
    let mut ids = BTreeSet::new();
    for segment in &session.segments {
        ids.insert(segment.speaker);
    }
    for turn in &session.turns {
        ids.insert(turn.speaker);
    }
    for speaker in ids {
        session.speakers.entry(speaker.to_string()).or_insert_with(|| SpeakerInfo {
            name: default_speaker_name(speaker),
        });
    }
}

pub fn new_id() -> String {
    Uuid::new_v4().to_string().replace('-', "").chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_json_falls_back_to_backup() {
        let root = std::env::temp_dir().join(format!("speaker-studio-save-{}", Uuid::new_v4()));
        let mut session = Session::new("Keep", "file");
        session.save(&root).unwrap();
        session.title = "Next".into();
        session.save(&root).unwrap();
        fs::write(session.dir(&root).join("session.json"), b"{not json").unwrap();
        let loaded = load_all(&root);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].title, "Keep");
        let _ = fs::remove_dir_all(root);
    }
}

pub fn video_ext(ext: &str) -> bool {
    matches!(ext, "mp4" | "webm" | "m4v" | "mov" | "ogv")
}

pub fn allowed_ext(ext: &str) -> bool {
    matches!(
        ext,
        "wav" | "mp3" | "m4a" | "aac" | "flac" | "ogg" | "opus" | "webm" | "mp4" | "m4v" | "mov" | "mkv" | "avi"
    )
}
