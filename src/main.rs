#![windows_subsystem = "windows"]

mod media;
mod session;

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rodio::Source;
use eframe::egui::{
    self, Button, Color32, CornerRadius, DragValue, Frame, Key, Margin, Pos2, Rect, RichText, ScrollArea, Sense, Stroke,
    TextEdit, TextureHandle, TextureOptions, Vec2,
};
use session::{
    allowed_ext, clean_name, clean_title, default_speaker_name, ensure_speakers, load_all, video_ext, Segment, Session,
    SpeakerInfo, Turn,
};

const BG0: Color32 = Color32::from_rgb(8, 11, 24);
const BG_CARD: Color32 = Color32::from_rgb(16, 22, 42);
const FIELD: Color32 = Color32::from_rgb(4, 7, 18);
const VIOLET: Color32 = Color32::from_rgb(152, 88, 255);
const CYAN: Color32 = Color32::from_rgb(85, 230, 238);
const TEXT: Color32 = Color32::from_rgb(243, 241, 255);
const MUTED: Color32 = Color32::from_rgb(165, 169, 195);
const DIM: Color32 = Color32::from_rgb(110, 116, 148);
const GOOD: Color32 = Color32::from_rgb(88, 225, 189);
const WARN: Color32 = Color32::from_rgb(245, 182, 66);
const BAD: Color32 = Color32::from_rgb(255, 111, 131);
const BORDER: Color32 = Color32::from_rgb(52, 55, 90);
const QUEUE_CAP: usize = 512;
const ME: u8 = 8;
const OVERLAP_SPEAKER: u8 = 254;
const UNASSIGNED_SPEAKER: u8 = 255;
const COLORS: [Color32; 8] = [
    Color32::from_rgb(85, 230, 238),
    Color32::from_rgb(168, 92, 255),
    Color32::from_rgb(255, 138, 61),
    Color32::from_rgb(245, 196, 66),
    Color32::from_rgb(255, 99, 132),
    Color32::from_rgb(106, 166, 255),
    Color32::from_rgb(186, 230, 72),
    Color32::from_rgb(255, 90, 196),
];

#[derive(Clone)]
struct EngineInfo {
    state: String,
    gpu: String,
    detail: String,
}

impl Default for EngineInfo {
    fn default() -> Self {
        Self { state: "loading".into(), gpu: String::new(), detail: "Starting the speech engine".into() }
    }
}

enum AppMsg {
    Log(String),
    Engine(EngineInfo),
    Status { job: String, message: String },
    Result { job: String, segments: Vec<Segment>, turns: Vec<Turn>, message: String, start: Option<f64>, end: Option<f64> },
    Live { job: String, segments: Vec<Segment>, turns: Vec<Turn> },
    Failed { job: String, message: String },
    Imported(Session),
    Note { id: String, message: String },
}

enum Page {
    Studio,
    Sessions,
}

struct Player {
    _stream: rodio::OutputStream,
    sink: rodio::Sink,
}

struct VideoPump {
    child: Child,
    latest: Arc<Mutex<Option<Vec<u8>>>>,
}

impl Drop for VideoPump {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

struct Mic {
    _stream: cpal::Stream,
    name: String,
}

struct Desktop {
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
    name: String,
}

impl Drop for Desktop {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[derive(Clone, Copy)]
enum AudioKind {
    Desktop,
    Mic,
}

#[derive(Clone)]
struct PcmOut {
    tx: mpsc::Sender<CapturePkt>,
    queued: Arc<AtomicUsize>,
    dropped: Arc<AtomicU64>,
    kind: AudioKind,
}

enum CapturePkt {
    Desktop(Vec<i16>),
    Mic(Vec<i16>),
    MicOn,
    MicOff,
}

struct CaptureMeters {
    party_peaks: Vec<f32>,
    mic_peaks: Vec<f32>,
    duration: f64,
    fault: Option<String>,
}

struct CaptureHub {
    tx: Option<mpsc::Sender<CapturePkt>>,
    meters: Arc<Mutex<CaptureMeters>>,
    queued: Arc<AtomicUsize>,
    dropped: Arc<AtomicU64>,
    join: Option<thread::JoinHandle<()>>,
}

impl Drop for CaptureHub {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct Studio {
    root: PathBuf,
    python: PathBuf,
    cmd: mpsc::Sender<String>,
    events: mpsc::Receiver<AppMsg>,
    event_tx: mpsc::Sender<AppMsg>,
    sessions: Vec<Session>,
    current: Option<String>,
    engine: EngineInfo,
    logs: Vec<String>,
    log_open: bool,
    page: Page,
    url: String,
    sel_start: f64,
    sel_end: f64,
    anchor: f64,
    max_speakers: u8,
    renaming: Option<u8>,
    rename_text: String,
    portrait_speaker: Option<u8>,
    portraits: HashMap<String, TextureHandle>,
    toast: String,
    toast_until: Option<Instant>,
    player: Option<Player>,
    video: Option<VideoPump>,
    video_tex: Option<TextureHandle>,
    play_at: f64,
    play_origin: Option<Instant>,
    followed_line: f64,
    paused: bool,
    mic_enabled: bool,
    output_device_id: String,
    output_devices: Vec<(String, String)>,
    mic: Option<Mic>,
    desktop: Option<Desktop>,
    hub: Option<CaptureHub>,
    capture_id: Option<String>,
    reported_drops: u64,
    last_save: Instant,
}

fn main() -> eframe::Result {
    if !claim_single_instance() {
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Speaker Studio")
            .with_inner_size([1480.0, 940.0])
            .with_min_inner_size([1100.0, 720.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("Speaker Studio", options, Box::new(|cc| Ok(Box::new(Studio::new(cc)))))
}

impl Studio {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        style_ui(&cc.egui_ctx);
        let root = project_root();
        let mut sessions = load_all(&root);
        let mut save_warning = String::new();
        for session in &mut sessions {
            if matches!(session.status.as_str(), "live" | "working" | "downloading") {
                session.status = "ready".into();
                session.message = "This was interrupted. Diarize it again when you want.".into();
                if let Err(error) = session.save(&root) {
                    save_warning = format!("Could not save this session ({error}).");
                }
            }
        }
        let (mic_enabled, output_device_id) = load_capture_settings(&root);
        let output_devices = list_output_devices();
        let (event_tx, events) = mpsc::channel();
        let cmd = spawn_engine(&root, event_tx.clone());
        let mut studio = Self {
            python: python_executable(&root),
            root,
            cmd,
            events,
            event_tx,
            sessions,
            current: None,
            engine: EngineInfo::default(),
            logs: Vec::new(),
            log_open: false,
            page: Page::Studio,
            url: String::new(),
            sel_start: 0.0,
            sel_end: 0.0,
            anchor: 0.0,
            max_speakers: 8,
            renaming: None,
            rename_text: String::new(),
            portrait_speaker: None,
            portraits: HashMap::new(),
            toast: String::new(),
            toast_until: None,
            player: None,
            video: None,
            video_tex: None,
            play_at: 0.0,
            play_origin: None,
            followed_line: -1.0,
            paused: true,
            mic_enabled,
            output_device_id,
            output_devices,
            mic: None,
            desktop: None,
            hub: None,
            capture_id: None,
            reported_drops: 0,
            last_save: Instant::now(),
        };
        if !save_warning.is_empty() {
            studio.toast(save_warning);
        }
        studio
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toast = text.into();
        self.toast_until = Some(Instant::now() + Duration::from_secs(4));
    }

    fn session(&self) -> Option<&Session> {
        let id = self.current.as_ref()?;
        self.sessions.iter().find(|session| &session.id == id)
    }

    fn session_mut(&mut self) -> Option<&mut Session> {
        let id = self.current.clone()?;
        self.sessions.iter_mut().find(|session| session.id == id)
    }

    fn store(&mut self, session: Session) {
        if let Err(error) = session.save(&self.root) {
            self.toast(format!("Could not save this session ({error})."));
        }
        if let Some(existing) = self.sessions.iter_mut().find(|item| item.id == session.id) {
            *existing = session;
        } else {
            self.sessions.insert(0, session);
        }
    }

    fn open_session(&mut self, id: &str) {
        self.stop_playback();
        self.current = Some(id.to_string());
        let (duration, max_speakers) = self.session().map(|session| (session.duration, session.max_speakers)).unwrap_or((0.0, 8));
        self.sel_start = 0.0;
        self.sel_end = duration;
        self.max_speakers = max_speakers;
        self.page = Page::Studio;
        self.video_tex = None;
        self.renaming = None;
        self.portrait_speaker = None;
        self.followed_line = -1.0;
    }

    fn send_cmd(&self, value: serde_json::Value) {
        let _ = self.cmd.send(value.to_string());
    }

    fn queue_diarize(&self, id: &str, wav: &Path, start: Option<f64>, end: Option<f64>) {
        self.send_cmd(serde_json::json!({
            "cmd": "diarize",
            "job": id,
            "wav": wav,
            "start": start,
            "end": end,
        }));
    }

    fn drain(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.events.try_recv() {
            self.apply(message, ctx);
        }
        self.drain_captures();
        self.drain_video(ctx);
        if let Some(until) = self.toast_until {
            if Instant::now() > until {
                self.toast.clear();
                self.toast_until = None;
            }
        }
        if self.play_origin.is_some() {
            if let Some(duration) = self.session().map(|session| session.duration) {
                if self.position() >= duration && duration > 0.0 {
                    self.stop_playback();
                    self.play_at = 0.0;
                }
            }
        }
    }

    fn apply(&mut self, message: AppMsg, _ctx: &egui::Context) {
        match message {
            AppMsg::Log(line) => self.push_log(line),
            AppMsg::Engine(info) => self.engine = info,
            AppMsg::Note { id, message } => {
                if let Some(session) = self.sessions.iter_mut().find(|session| session.id == id) {
                    session.message = message;
                }
            }
            AppMsg::Status { job, message } => {
                if !message.is_empty() {
                    self.push_log(message.clone());
                }
                if job.is_empty() {
                    if self.engine.state != "ready" {
                        self.engine.detail = short_line(&message);
                    }
                } else if let Some(session) = self.sessions.iter_mut().find(|session| session.id == job) {
                    session.message = short_line(&message);
                }
            }
            AppMsg::Result { job, segments, turns, message, start, end } => {
                if let Some(mut session) = self.sessions.iter().find(|session| session.id == job).cloned() {
                    if let (Some(start), Some(end)) = (start, end) {
                        merge_selection(&mut session, segments, turns, start, end);
                    } else {
                        session.segments = segments;
                        session.turns = turns;
                    }
                    ensure_speakers(&mut session);
                    session.status = "done".into();
                    session.message = if message.is_empty() { "Transcript ready".into() } else { message };
                    self.store(session);
                }
            }
            AppMsg::Live { job, segments, turns } => {
                if let Some(mut session) = self.sessions.iter().find(|session| session.id == job).cloned() {
                    if session.status != "live" {
                        return;
                    }
                    session.segments = segments;
                    session.turns = turns;
                    ensure_speakers(&mut session);
                    session.message = if self.mic.is_some() {
                        "Party on the desktop audio, you on the microphone. Words appear a few seconds later.".into()
                    } else {
                        "Party on the desktop audio. Microphone is off. Words appear a few seconds later.".into()
                    };
                    self.store(session);
                }
            }
            AppMsg::Failed { job, message } => {
                self.push_log(message.clone());
                if job.is_empty() {
                    self.engine.state = "error".into();
                    self.engine.detail = short_line(&message);
                } else if let Some(mut session) = self.sessions.iter().find(|session| session.id == job).cloned() {
                    session.status = "error".into();
                    session.message = short_line(&message);
                    self.store(session);
                } else {
                    self.toast(short_line(&message));
                }
            }
            AppMsg::Imported(mut session) => {
                let id = session.id.clone();
                let wav = session.dir(&self.root).join("audio.wav");
                session.status = "working".into();
                session.message = "Diarizing with Nemotron 3".into();
                self.store(session);
                self.open_session(&id);
                self.queue_diarize(&id, &wav, None, None);
            }
        }
    }

    fn push_log(&mut self, line: String) {
        self.logs.push(line);
        if self.logs.len() > 300 {
            self.logs.drain(..50);
        }
    }

    fn import_path(&mut self, path: PathBuf) {
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase();
        if !allowed_ext(&ext) {
            self.toast("Use a video or audio file.");
            return;
        }
        let mut session = Session::new(&file_title(&path), "file");
        session.status = "working".into();
        session.message = "Reading audio".into();
        let dir = session.dir(&self.root);
        if std::fs::create_dir_all(&dir).is_err() {
            self.toast("Could not create a session folder.");
            return;
        }
        self.store(session.clone());
        self.open_session(&session.id);
        let tx = self.event_tx.clone();
        let root = self.root.clone();
        let id = session.id.clone();
        thread::spawn(move || {
            let message = match prepare_import(&root, &id, &path, &ext) {
                Ok(session) => AppMsg::Imported(session),
                Err(error) => AppMsg::Failed { job: id, message: error },
            };
            let _ = tx.send(message);
        });
    }

    fn open_link(&mut self) {
        let url = self.url.trim().to_string();
        if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() > 2000 {
            self.toast("Paste a full http or https link.");
            return;
        }
        if !self.python.exists() {
            self.toast("Run start.ps1 once so the speech engine can install.");
            return;
        }
        self.url.clear();
        let mut session = Session::new("Opening link", "url");
        session.status = "downloading".into();
        session.message = "Downloading".into();
        let dir = session.dir(&self.root);
        if std::fs::create_dir_all(&dir).is_err() {
            self.toast("Could not create a session folder.");
            return;
        }
        self.store(session.clone());
        self.open_session(&session.id);
        let tx = self.event_tx.clone();
        let root = self.root.clone();
        let python = self.python.clone();
        let id = session.id.clone();
        thread::spawn(move || {
            let message = match download_link(&root, &python, &id, &url, &tx) {
                Ok(session) => AppMsg::Imported(session),
                Err(error) => AppMsg::Failed { job: id, message: error },
            };
            let _ = tx.send(message);
        });
    }

    fn toggle_live(&mut self) {
        if self.capturing() {
            self.stop_live();
            return;
        }
        self.stop_playback();
        let mut session = Session::new(&format!("Live {}", chrono::Utc::now().format("%b %d %H:%M")), "live");
        session.visual = "audio".into();
        session.media_file = "audio.wav".into();
        session.status = "live".into();
        let dir = session.dir(&self.root);
        if std::fs::create_dir_all(&dir).is_err() {
            self.toast("Could not create a session folder.");
            return;
        }
        let party_wav = match media::LiveWav::create(&dir.join("desktop.wav")) {
            Ok(wav) => wav,
            Err(error) => {
                self.toast(error.to_string());
                return;
            }
        };
        let mic_wav = if self.mic_enabled {
            match media::LiveWav::create(&dir.join("mic.wav")) {
                Ok(wav) => Some(wav),
                Err(error) => {
                    self.toast(error.to_string());
                    None
                }
            }
        } else {
            None
        };
        let id = session.id.clone();
        let hub = spawn_capture_hub(id.clone(), dir.clone(), party_wav, mic_wav, self.cmd.clone());
        self.send_cmd(serde_json::json!({"cmd": "live_start", "job": id}));
        let desktop = match start_desktop(self.output_device_id.clone(), self.output_device_name(), hub.output(AudioKind::Desktop)) {
            Ok(desktop) => Some(desktop),
            Err(error) => {
                self.toast(format!("Desktop audio: {error}"));
                None
            }
        };
        let mic = if self.mic_enabled {
            match start_mic(hub.output(AudioKind::Mic)) {
                Ok(mic) => Some(mic),
                Err(error) => {
                    self.toast(format!("Microphone: {error}"));
                    None
                }
            }
        } else {
            None
        };
        if mic.is_none() && desktop.is_none() {
            self.send_cmd(serde_json::json!({"cmd": "live_stop", "job": id}));
            return;
        }
        let mic_name = mic.as_ref().map(|item| item.name.clone()).unwrap_or_else(|| "unavailable".into());
        let desktop_name = desktop.as_ref().map(|item| item.name.clone()).unwrap_or_else(|| "unavailable".into());
        session.message = if self.mic_enabled {
            format!("Party · {desktop_name}. You · {mic_name}. Words appear a few seconds later.")
        } else {
            format!("Party · {desktop_name}. Microphone is off. Words appear a few seconds later.")
        };
        if self.mic_enabled {
            session.speakers.insert("8".into(), SpeakerInfo { name: "Me".into() });
        }
        self.capture_id = Some(id.clone());
        self.reported_drops = 0;
        self.mic = mic;
        self.desktop = desktop;
        self.hub = Some(hub);
        self.store(session);
        self.open_session(&id);
        if let Some(session) = self.session_mut() {
            session.status = "live".into();
        }
    }

    fn stop_live(&mut self) {
        let id = self.capture_id.clone();
        self.mic = None;
        self.desktop = None;
        self.hub = None;
        self.capture_id = None;
        let Some(id) = id else { return };
        let dir = self.root.join("sessions").join(&id);
        let desktop = dir.join("desktop.wav");
        let mic = dir.join("mic.wav");
        let audio = dir.join("audio.wav");
        let _ = mix_wavs(&desktop, &mic, &audio);
        if let Some(mut session) = self.sessions.iter().find(|session| session.id == id).cloned() {
            if let Ok((duration, peaks)) = media::peaks_from_wav(&audio, 2000) {
                session.duration = duration;
                session.peaks = peaks;
            }
            session.status = "working".into();
            session.message = "Separating you from the party".into();
            if self.current.as_deref() == Some(id.as_str()) {
                self.sel_end = session.duration;
            }
            self.store(session);
        }
        self.send_cmd(serde_json::json!({"cmd": "live_stop", "job": id}));
        self.send_cmd(serde_json::json!({
            "cmd": "refine_sources",
            "job": id,
            "desktop": desktop,
            "mic": mic,
        }));
    }

    fn drain_captures(&mut self) {
        let Some(id) = self.capture_id.clone() else { return };
        let Some(hub) = &self.hub else { return };
        let dropped = hub.dropped.load(Ordering::Relaxed);
        let meters = hub.meters.lock().ok().map(|mut meters| {
            (meters.party_peaks.clone(), meters.mic_peaks.clone(), meters.duration, meters.fault.take())
        });
        let Some((party_peaks, mic_peaks, duration, fault)) = meters else { return };
        if dropped > self.reported_drops {
            let count = dropped - self.reported_drops;
            self.reported_drops = dropped;
            self.toast(format!("Live capture fell behind. {count} audio packets were dropped."));
        }
        if let Some(message) = fault {
            self.toast(message);
        }
        if let Some(session) = self.sessions.iter_mut().find(|session| session.id == id && session.status == "live") {
            session.party_peaks = party_peaks.clone();
            session.peaks = party_peaks;
            session.mic_peaks = mic_peaks;
            session.duration = session.duration.max(duration);
        }
        if self.current.as_deref() == Some(id.as_str()) {
            if let Some(session) = self.sessions.iter().find(|session| session.id == id) {
                self.sel_end = session.duration;
            }
        }
        if self.last_save.elapsed() >= Duration::from_secs(2) {
            if let Some(session) = self.sessions.iter().find(|session| session.id == id).cloned() {
                if let Err(error) = session.save(&self.root) {
                    self.toast(format!("Could not save this session ({error})."));
                }
            }
            self.last_save = Instant::now();
        }
    }

    fn diarize_selection(&mut self) {
        if self.capturing() {
            self.toast("Stop live dictation before diarizing a selection.");
            return;
        }
        let Some(mut session) = self.session().cloned() else {
            self.toast("Open a recording first.");
            return;
        };
        let audio = session.dir(&self.root).join("audio.wav");
        if !audio.exists() {
            self.toast("This session has no audio yet.");
            return;
        }
        let start = self.sel_start.max(0.0);
        let end = self.sel_end.max(start);
        let full = start <= 0.05 && end >= session.duration - 0.05;
        session.status = "working".into();
        session.message = if full { "Diarizing with Nemotron 3".into() } else { "Diarizing the selection".into() };
        let id = session.id.clone();
        self.store(session);
        let start_arg = if full { None } else { Some(start) };
        let end_arg = if full { None } else { Some(end) };
        self.queue_diarize(&id, &audio, start_arg, end_arg);
    }

    fn play(&mut self) {
        if self.capturing() {
            return;
        }
        if self.player.is_some() && self.paused {
            if let Some(player) = &self.player {
                player.sink.play();
            }
            self.play_origin = Some(Instant::now());
            self.paused = false;
            self.start_video();
            return;
        }
        if self.player.is_some() && !self.paused {
            return;
        }
        let start = if self.play_at > 0.0 { self.play_at } else { self.sel_start };
        self.play_from(start);
    }

    fn pause(&mut self) {
        self.play_at = self.position();
        if let Some(player) = &self.player {
            player.sink.pause();
        }
        self.play_origin = None;
        self.paused = true;
        self.video = None;
    }

    fn play_from(&mut self, at: f64) {
        self.stop_playback();
        let Some(session) = self.session().cloned() else { return };
        let wav = session.dir(&self.root).join("audio.wav");
        if !wav.exists() {
            self.toast("This session has no audio yet.");
            return;
        }
        match open_player(&wav, at) {
            Ok(player) => {
                self.player = Some(player);
                self.play_at = at;
                self.play_origin = Some(Instant::now());
                self.paused = false;
                self.start_video();
            }
            Err(error) => self.toast(error),
        }
    }

    fn stop_playback(&mut self) {
        self.player = None;
        self.video = None;
        self.play_origin = None;
        self.paused = true;
    }

    fn seek(&mut self, at: f64) {
        let duration = self.session().map(|session| session.duration).unwrap_or(0.0);
        let at = if duration > 0.0 { at.clamp(0.0, duration) } else { at.max(0.0) };
        let had_player = self.player.is_some();
        let was_paused = self.paused;
        if had_player {
            self.play_from(at);
            if was_paused {
                if let Some(player) = &self.player {
                    player.sink.pause();
                }
                self.play_origin = None;
                self.paused = true;
                self.video = None;
                self.play_at = at;
            }
        } else {
            self.play_at = at;
        }
    }

    fn position(&self) -> f64 {
        match self.play_origin {
            Some(origin) if !self.paused => self.play_at + origin.elapsed().as_secs_f64(),
            _ => self.play_at,
        }
    }

    fn start_video(&mut self) {
        let Some(session) = self.session().cloned() else { return };
        if session.visual != "video" {
            self.video = None;
            return;
        }
        let media = session.dir(&self.root).join(&session.media_file);
        self.video = VideoPump::start(&media, self.position());
    }

    fn drain_video(&mut self, ctx: &egui::Context) {
        let frame = self.video.as_ref().and_then(|video| video.latest.lock().ok().and_then(|mut slot| slot.take()));
        let Some(frame) = frame else { return };
        let image = egui::ColorImage::from_rgba_unmultiplied([480, 270], &frame);
        if let Some(texture) = &mut self.video_tex {
            texture.set(image, TextureOptions::LINEAR);
        } else {
            self.video_tex = Some(ctx.load_texture("preview", image, TextureOptions::LINEAR));
        }
    }

    fn save_name(&mut self) {
        let Some(speaker) = self.renaming else { return };
        let name = clean_name(&self.rename_text, speaker);
        if let Some(mut session) = self.session().cloned() {
            session.speakers.insert(speaker.to_string(), SpeakerInfo { name });
            self.store(session);
        }
        self.renaming = None;
    }

    fn set_mic_enabled(&mut self, enabled: bool) {
        self.mic_enabled = enabled;
        save_capture_settings(&self.root, enabled, &self.output_device_id);
        if self.capture_id.is_none() {
            return;
        }
        if enabled {
            if self.mic.is_none() {
                if let Some(hub) = &self.hub {
                    hub.send_control(CapturePkt::MicOn);
                    match start_mic(hub.output(AudioKind::Mic)) {
                        Ok(mic) => self.mic = Some(mic),
                        Err(error) => self.toast(format!("Microphone: {error}")),
                    }
                }
            }
            return;
        }
        self.mic = None;
        if let Some(hub) = &self.hub {
            hub.send_control(CapturePkt::MicOff);
        }
        if let Some(id) = &self.capture_id {
            if let Some(session) = self.sessions.iter_mut().find(|session| &session.id == id) {
                if session.status == "live" {
                    session.message = "Me is off. Earlier microphone audio stays on the session clock.".into();
                }
            }
        }
    }

    fn capturing(&self) -> bool {
        self.capture_id.is_some()
    }

    fn visible(&self, speaker: u8) -> bool {
        match speaker {
            ME => self.mic_enabled,
            OVERLAP_SPEAKER | UNASSIGNED_SPEAKER => true,
            _ => speaker < self.max_speakers,
        }
    }

    fn speaker_name(&self, speaker: u8) -> String {
        self.session()
            .and_then(|session| session.speakers.get(&speaker.to_string()).map(|info| info.name.clone()))
            .unwrap_or_else(|| default_speaker_name(speaker))
    }

    fn portrait_target(&self) -> Option<u8> {
        self.portrait_speaker.or(self.renaming)
    }

    fn portrait_path(&self, speaker: u8) -> Option<PathBuf> {
        let id = self.current.as_ref()?;
        Some(self.root.join("sessions").join(id).join("portraits").join(format!("{speaker}.png")))
    }

    fn portrait_texture(&mut self, ctx: &egui::Context, speaker: u8) -> Option<TextureHandle> {
        let id = self.current.clone()?;
        let key = format!("{id}:{speaker}");
        if let Some(texture) = self.portraits.get(&key) {
            return Some(texture.clone());
        }
        let path = self.portrait_path(speaker)?;
        let bytes = std::fs::read(path).ok()?;
        let image = image::load_from_memory(&bytes).ok()?.into_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
        let texture = ctx.load_texture(key.clone(), color, TextureOptions::LINEAR);
        self.portraits.insert(key, texture.clone());
        Some(texture)
    }

    fn save_portrait_rgba(&mut self, speaker: u8, width: u32, height: u32, bytes: &[u8]) -> Result<(), String> {
        let mut image = image::RgbaImage::from_raw(width, height, bytes.to_vec()).ok_or("That image could not be read.")?;
        let max_side = 512u32;
        let longest = image.width().max(image.height());
        if longest > max_side {
            let scale = max_side as f32 / longest as f32;
            let w = ((image.width() as f32) * scale).round().max(1.0) as u32;
            let h = ((image.height() as f32) * scale).round().max(1.0) as u32;
            image = image::imageops::resize(&image, w, h, image::imageops::FilterType::Triangle);
        }
        let path = self.portrait_path(speaker).ok_or("Open a session first.")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        image.save(&path).map_err(|error| error.to_string())?;
        if let Some(id) = &self.current {
            self.portraits.remove(&format!("{id}:{speaker}"));
        }
        self.portrait_speaker = Some(speaker);
        Ok(())
    }

    fn paste_portrait(&mut self) {
        let Some(speaker) = self.portrait_target() else {
            self.toast("Click a speaker's picture first, then paste.");
            return;
        };
        let image = match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_image()) {
            Ok(image) => image,
            Err(_) => {
                self.toast("Copy an image first. In Discord, right-click an avatar and choose Copy Image.");
                return;
            }
        };
        let width = image.width as u32;
        let height = image.height as u32;
        if let Err(error) = self.save_portrait_rgba(speaker, width, height, &image.bytes) {
            self.toast(error);
            return;
        }
        let name = self.speaker_name(speaker);
        self.toast(format!("Picture saved for {name}."));
    }

    fn save_portrait_file(&mut self, speaker: u8, path: &Path) {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.toast(error.to_string());
                return;
            }
        };
        let image = match image::load_from_memory(&bytes) {
            Ok(image) => image.into_rgba8(),
            Err(_) => {
                self.toast("That file is not a picture.");
                return;
            }
        };
        if let Err(error) = self.save_portrait_rgba(speaker, image.width(), image.height(), image.as_raw()) {
            self.toast(error);
        } else {
            let name = self.speaker_name(speaker);
            self.toast(format!("Picture saved for {name}."));
        }
    }

    fn export_page(&mut self) {
        let Some(session) = self.session().cloned() else {
            self.toast("Open a session first.");
            return;
        };
        let filename = format!("{}.html", session.title.replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], ""));
        let Some(path) = rfd::FileDialog::new().set_file_name(filename).add_filter("Web page", &["html"]).save_file() else {
            return;
        };
        match write_transcript_page(&self.root, &session, &path) {
            Ok(()) => self.toast("Exported a page you can send to someone."),
            Err(error) => self.toast(error),
        }
    }
}

impl eframe::App for Studio {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain(ctx);
        ctx.request_repaint_after(Duration::from_millis(33));
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input.raw.dropped_files.iter().filter_map(|file| file.path.clone()).collect()
        });
        for path in dropped {
            let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase();
            if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "bmp") {
                if let Some(speaker) = self.portrait_target() {
                    self.save_portrait_file(speaker, &path);
                } else {
                    self.toast("Click a speaker's picture first, then drop the image.");
                }
            } else {
                self.import_path(path);
            }
        }
        if !ctx.wants_keyboard_input() && ctx.input(|input| input.modifiers.command && input.key_pressed(Key::V)) {
            self.paste_portrait();
        }
        if !ctx.wants_keyboard_input() && ctx.input(|input| input.key_pressed(Key::Space)) {
            if self.paused { self.play(); } else { self.pause(); }
        }
        egui::TopBottomPanel::top("bar").exact_height(58.0).show(ctx, |ui| self.top_bar(ui));
        if self.log_open {
            egui::SidePanel::left("log").exact_width(340.0).show(ctx, |ui| self.log_panel(ui));
        }
        egui::CentralPanel::default().show(ctx, |ui| match self.page {
            Page::Studio => self.studio_page(ui, ctx),
            Page::Sessions => self.sessions_page(ui),
        });
        if !self.toast.is_empty() {
            egui::TopBottomPanel::bottom("toast").show(ctx, |ui| {
                ui.colored_label(TEXT, &self.toast);
            });
        }
    }
}

impl Studio {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.label(RichText::new("SPEAKER STUDIO").color(CYAN).strong());
            ui.add_space(12.0);
            if ui.selectable_label(matches!(self.page, Page::Studio), "Studio").clicked() {
                self.page = Page::Studio;
            }
            if ui.selectable_label(matches!(self.page, Page::Sessions), "Sessions").clicked() {
                self.page = Page::Sessions;
            }
            ui.add_space(8.0);
            pill(ui, &self.engine);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.selectable_label(self.log_open, "Log").clicked() {
                    self.log_open = !self.log_open;
                }
                ui.label(RichText::new("LOCAL NEMOTRON DIARIZATION").color(DIM).size(10.0));
            });
        });
    }

    fn output_device_name(&self) -> String {
        self.output_devices
            .iter()
            .find(|(id, _)| id == &self.output_device_id)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| "Windows default output".into())
    }

    fn set_output_device(&mut self, id: String) {
        if self.output_device_id == id {
            return;
        }
        self.output_device_id = id;
        save_capture_settings(&self.root, self.mic_enabled, &self.output_device_id);
        if self.desktop.is_some() {
            let output = self.hub.as_ref().map(|hub| hub.output(AudioKind::Desktop));
            self.desktop = None;
            if let Some(output) = output {
                match start_desktop(self.output_device_id.clone(), self.output_device_name(), output) {
                    Ok(desktop) => self.desktop = Some(desktop),
                    Err(error) => self.toast(format!("Could not switch output: {error}")),
                }
            }
        }
    }

    fn studio_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(8.0);
            ui.columns(2, |columns| {
                card(&mut columns[0], |ui| {
                    ui.label(RichText::new("SOURCE").color(DIM).size(11.0));
                    ui.label(RichText::new("Drop a video or audio file").strong().size(18.0));
                    ui.label(RichText::new("MP4, MKV, WebM, MOV, WAV, MP3, M4A. It stays on this computer.").color(MUTED));
                    if ui.button("Choose a file").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Media", &["mp4", "mkv", "webm", "mov", "wav", "mp3", "m4a", "flac", "ogg", "m4v", "avi"])
                            .pick_file()
                        {
                            self.import_path(path);
                        }
                    }
                });
                card(&mut columns[1], |ui| {
                    ui.label(RichText::new("LINK").color(DIM).size(11.0));
                    ui.label(RichText::new("YouTube or a video URL").strong().size(18.0));
                    ui.add(TextEdit::singleline(&mut self.url).desired_width(f32::INFINITY));
                    ui.horizontal(|ui| {
                        if ui.add(primary("Open")).clicked() {
                            self.open_link();
                        }
                        let live = self.capturing();
                        let label = if live { "Stop dictation" } else { "Live dictation" };
                        if ui.add(primary(label)).clicked() {
                            self.toggle_live();
                        }
                        let mut mic_on = self.mic_enabled;
                        if ui.checkbox(&mut mic_on, "Me").changed() {
                            self.set_mic_enabled(mic_on);
                        }
                    });
                });
            });
            ui.add_space(8.0);
            card(ui, |ui| {
                ui.label(RichText::new("OUTPUT").color(DIM).size(11.0));
                ui.label(RichText::new("Which headphones or speakers Live dictation captures").color(MUTED));
                let selected = self.output_device_name();
                let devices = self.output_devices.clone();
                egui::ComboBox::from_id_salt("output-device").selected_text(selected).width(420.0).show_ui(ui, |ui| {
                    if ui.selectable_label(self.output_device_id.is_empty(), "Windows default output").clicked() {
                        self.set_output_device(String::new());
                    }
                    for (id, name) in devices {
                        if ui.selectable_label(&self.output_device_id == &id, &name).clicked() {
                            self.set_output_device(id);
                        }
                    }
                });
            });
            ui.add_space(10.0);
            self.stage(ui);
            ui.add_space(8.0);
            self.transport(ui);
            ui.add_space(10.0);
            self.lanes_and_transcript(ui, ctx);
        });
    }

    fn stage(&mut self, ui: &mut egui::Ui) {
        let duration = self.session().map(|session| session.duration).unwrap_or(0.0);
        let title = self.session().map(|session| session.title.clone()).unwrap_or_else(|| "No recording yet".into());
        let kind = self.session().map(|session| session.kind.clone()).unwrap_or_else(|| "Studio".into());
        let message = self.session().map(|session| session.message.clone()).unwrap_or_else(|| {
            if self.engine.state == "ready" {
                "Drop a file, paste a link, or start live dictation.".into()
            } else {
                self.engine.detail.clone()
            }
        });
        let peaks = self.session().map(|session| session.peaks.clone()).unwrap_or_default();
        let party_peaks = self.session().map(|session| session.party_peaks.clone()).unwrap_or_default();
        let mic_peaks = self.session().map(|session| session.mic_peaks.clone()).unwrap_or_default();
        let segments = self.session().map(|session| session.segments.clone()).unwrap_or_default();
        let max_speakers = self.max_speakers;
        let video = self.session().map(|session| session.visual == "video").unwrap_or(false);
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(kind.to_uppercase()).color(DIM).size(11.0));
                    ui.label(RichText::new(title).strong().size(26.0));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.label(RichText::new(format!("{}   {:.1} seconds selected", stamp(duration), (self.sel_end - self.sel_start).max(0.0))).color(DIM));
                });
            });
            if video {
                if let Some(texture) = &self.video_tex {
                    ui.add_space(8.0);
                    ui.image((texture.id(), Vec2::new(480.0, 270.0)));
                }
            }
            let split = self.mic_enabled && !party_peaks.is_empty();
            let (rect, response) = if split {
                ui.label(RichText::new("PARTY").color(DIM).size(11.0));
                let (party_rect, party_response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 78.0), Sense::click_and_drag());
                let party_segments: Vec<Segment> = segments.iter().filter(|segment| segment.speaker != 8).cloned().collect();
                paint_wave(ui, party_rect, &party_peaks, &party_segments, max_speakers, self.sel_start, self.sel_end, self.position(), duration);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("ME").color(voice_color(8)).size(11.0));
                    if ui.button("Turn off").clicked() {
                        self.set_mic_enabled(false);
                    }
                });
                let (mic_rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), Sense::click());
                let mic_segments: Vec<Segment> = segments.iter().filter(|segment| segment.speaker == 8).cloned().collect();
                paint_wave(ui, mic_rect, &mic_peaks, &mic_segments, 9, self.sel_start, self.sel_end, self.position(), duration);
                (party_rect, party_response)
            } else {
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 168.0), Sense::click_and_drag());
                paint_wave(ui, rect, &peaks, &segments, max_speakers, self.sel_start, self.sel_end, self.position(), duration);
                (rect, response)
            };
            if response.drag_started() {
                self.anchor = time_at(rect, response.interact_pointer_pos().unwrap_or(rect.left_top()), duration);
            }
            if response.dragged() {
                let at = time_at(rect, response.interact_pointer_pos().unwrap_or(rect.left_top()), duration);
                self.sel_start = self.anchor.min(at);
                self.sel_end = self.anchor.max(at);
            }
            if response.clicked() {
                let at = time_at(rect, response.interact_pointer_pos().unwrap_or(rect.left_top()), duration);
                self.sel_start = 0.0;
                self.sel_end = duration;
                self.seek(at);
            }
            ui.add_space(6.0);
            ui.label(RichText::new(message).color(MUTED));
        });
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        let duration = self.session().map(|session| session.duration).unwrap_or(0.0);
        ui.horizontal(|ui| {
            if ui.button("Play").clicked() { self.play(); }
            if ui.button("Pause").clicked() { self.pause(); }
            if ui.button("Stop").clicked() {
                let start = self.sel_start;
                self.stop_playback();
                self.play_at = start;
            }
            ui.label("Start");
            ui.add(DragValue::new(&mut self.sel_start).speed(0.1).range(0.0..=duration.max(0.0)).fixed_decimals(1));
            ui.label("End");
            ui.add(DragValue::new(&mut self.sel_end).speed(0.1).range(0.0..=duration.max(0.0)).fixed_decimals(1));
            ui.label("Show speakers");
            let previous = self.max_speakers;
            ui.add(DragValue::new(&mut self.max_speakers).range(1..=8));
            if self.max_speakers != previous {
                if let Some(mut session) = self.session().cloned() {
                    session.max_speakers = self.max_speakers;
                    self.store(session);
                }
            }
            if ui.add(primary("Diarize selection")).clicked() { self.diarize_selection(); }
            if ui.button("Copy text").clicked() { self.copy_text(ui.ctx()); }
            if ui.button("Download").clicked() { self.download_text(); }
            if ui.button("Export page").clicked() { self.export_page(); }
        });
        if self.sel_end < self.sel_start {
            self.sel_end = self.sel_start;
        }
    }

    fn lanes_and_transcript(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let duration = self.session().map(|session| session.duration).unwrap_or(0.0);
        let segments = self.session().map(|session| session.segments.clone()).unwrap_or_default();
        let turns = self.session().map(|session| session.turns.clone()).unwrap_or_default();
        let mut speakers: Vec<u8> = segments.iter().map(|segment| segment.speaker).chain(turns.iter().map(|turn| turn.speaker)).filter(|speaker| self.visible(*speaker)).collect();
        if self.mic_enabled {
            speakers.push(8);
        }
        speakers.sort_unstable();
        speakers.dedup();
        let names: Vec<(u8, String)> = speakers.iter().copied().map(|speaker| (speaker, self.speaker_name(speaker))).collect();
        let position = self.position();
        ui.columns(2, |columns| {
            card(&mut columns[0], |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("SPEAKER ACTIVITY").color(DIM).size(11.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(if names.is_empty() { "Who spoke when".into() } else { format!("{} speakers", names.len()) }).color(DIM));
                    });
                });
                if names.is_empty() {
                    ui.add_space(24.0);
                    ui.label(RichText::new("Speaker lanes appear here").color(MUTED));
                    return;
                }
                ScrollArea::vertical().max_height(420.0).id_salt("lanes").show(ui, |ui| {
                    for (speaker, name) in &names {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let color = voice_color(*speaker);
                            let chosen = self.portrait_speaker == Some(*speaker);
                            if let Some(texture) = self.portrait_texture(ctx, *speaker) {
                                let response = ui.add(egui::Image::new(&texture).fit_to_exact_size(Vec2::splat(32.0)).corner_radius(CornerRadius::same(16)).sense(Sense::click()));
                                if chosen {
                                    ui.painter().rect_stroke(response.rect, CornerRadius::same(16), Stroke::new(2.0, color), egui::StrokeKind::Outside);
                                }
                                if response.clicked() {
                                    self.portrait_speaker = Some(*speaker);
                                }
                            } else {
                                let (swatch, response) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
                                ui.painter().circle_filled(swatch.center(), 14.0, color);
                                if chosen {
                                    ui.painter().circle_stroke(swatch.center(), 15.0, Stroke::new(2.0, TEXT));
                                }
                                if response.clicked() {
                                    self.portrait_speaker = Some(*speaker);
                                }
                            }
                            if self.renaming == Some(*speaker) {
                                let edit = ui.add(TextEdit::singleline(&mut self.rename_text).desired_width(180.0).hint_text("Greg - Ruthgar the Invincible - Barbarian"));
                                if edit.lost_focus() {
                                    if ui.input(|input| input.key_pressed(Key::Escape)) {
                                        self.renaming = None;
                                    } else {
                                        self.save_name();
                                    }
                                }
                            } else if ui.add(Button::new(RichText::new(name).color(color)).frame(false)).clicked() {
                                self.renaming = Some(*speaker);
                                self.rename_text = name.clone();
                            }
                            if ui.button("Paste").clicked() {
                                self.portrait_speaker = Some(*speaker);
                                self.paste_portrait();
                            }
                            if *speaker == 8 && ui.button("Turn off").clicked() {
                                self.set_mic_enabled(false);
                            }
                            let (track, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
                            ui.painter().rect_filled(track, 5.0, FIELD);
                            if duration > 0.0 {
                                for segment in segments.iter().filter(|segment| segment.speaker == *speaker) {
                                    let x = track.left() + (segment.start / duration) as f32 * track.width();
                                    let w = ((segment.end - segment.start) / duration).max(0.0) as f32 * track.width();
                                    let bar = Rect::from_min_size(Pos2::new(x, track.top() + 3.0), Vec2::new(w.max(2.0), track.height() - 6.0));
                                    ui.painter().rect_filled(bar, 3.0, color);
                                }
                                let head = track.left() + (position / duration).clamp(0.0, 1.0) as f32 * track.width();
                                ui.painter().vline(head, track.y_range(), Stroke::new(1.0, CYAN));
                            }
                        });
                    }
                });
            });
            card(&mut columns[1], |ui| {
                let shown: Vec<&Turn> = turns.iter().filter(|turn| self.visible(turn.speaker)).collect();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("DIARIZATION + ASR").color(DIM).size(11.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(if shown.is_empty() { "Text for each speaker".into() } else { format!("{} lines", shown.len()) }).color(DIM));
                    });
                });
                if shown.is_empty() {
                    ui.add_space(24.0);
                    ui.label(RichText::new("Transcript lines appear here").color(MUTED));
                    return;
                }
                let listening = self.capturing();
                let playing = self.play_origin.is_some();
                let last_start = shown.last().map(|turn| turn.start);
                ScrollArea::vertical().max_height(420.0).stick_to_bottom(listening).id_salt("turns").show(ui, |ui| {
                    for turn in &shown {
                        let name = self.speaker_name(turn.speaker);
                        let color = voice_color(turn.speaker);
                        let active = position >= turn.start && position < turn.end + 0.35;
                        let frame = if active { Color32::from_rgba_unmultiplied(152, 88, 255, 48) } else { Color32::TRANSPARENT };
                        let row = Frame::NONE.fill(frame).inner_margin(Margin::symmetric(6, 4)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if let Some(texture) = self.portrait_texture(ctx, turn.speaker) {
                                    ui.add(egui::Image::new(&texture).fit_to_exact_size(Vec2::splat(28.0)).corner_radius(CornerRadius::same(14)));
                                } else {
                                    let (swatch, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                                    ui.painter().circle_filled(swatch.center(), 12.0, color);
                                }
                                ui.label(RichText::new(&name).color(color).strong());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(RichText::new(stamp(turn.start)).color(DIM).size(11.0));
                                });
                            });
                            if ui.add(Button::new(RichText::new(&turn.text).color(TEXT)).frame(false).wrap()).clicked() {
                                self.seek(turn.start);
                            }
                        });
                        let keep_up = listening && Some(turn.start) == last_start;
                        let entered = playing && active && (self.followed_line - turn.start).abs() > 0.05;
                        if keep_up {
                            row.response.scroll_to_me(Some(egui::Align::BOTTOM));
                        } else if entered {
                            row.response.scroll_to_me(Some(egui::Align::Center));
                            self.followed_line = turn.start;
                        }
                    }
                });
            });
        });
    }

    fn sessions_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(RichText::new("LIBRARY").color(DIM).size(11.0));
        ui.label(RichText::new("Sessions").strong().size(26.0));
        ui.label(RichText::new(format!("{} sessions", self.sessions.len())).color(DIM));
        ui.add_space(8.0);
        if self.sessions.is_empty() {
            ui.label(RichText::new("No sessions yet. Drop a recording or start live dictation.").color(MUTED));
            return;
        }
        ScrollArea::vertical().show(ui, |ui| {
            let rows: Vec<(String, String, String, f64)> = self.sessions.iter().map(|session| (session.id.clone(), session.title.clone(), session.message.clone(), session.duration)).collect();
            for (id, title, message, duration) in rows {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(title).strong());
                            ui.label(RichText::new(format!("{message} · {}", stamp(duration))).color(MUTED).size(12.0));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Delete").clicked() {
                                self.delete_session(&id);
                            }
                            if ui.button("Open").clicked() {
                                self.open_session(&id);
                            }
                        });
                    });
                });
                ui.add_space(8.0);
            }
        });
    }

    fn delete_session(&mut self, id: &str) {
        if self.capture_id.as_deref() == Some(id) {
            self.toast("Stop live dictation before deleting it.");
            return;
        }
        self.sessions.retain(|session| session.id != id);
        let _ = std::fs::remove_dir_all(self.root.join("sessions").join(id));
        if self.current.as_deref() == Some(id) {
            self.stop_playback();
            self.current = None;
        }
    }

    fn copy_text(&mut self, ctx: &egui::Context) {
        let text = self.selection_text();
        if text.is_empty() {
            self.toast("No transcript in the selection yet.");
            return;
        }
        ctx.copy_text(text);
        self.toast("Copied the selection.");
    }

    fn download_text(&mut self) {
        let Some(session) = self.session().cloned() else {
            self.toast("No transcript yet.");
            return;
        };
        let body = transcript_text(&session, 0.0, session.duration.max(self.sel_end), self.max_speakers);
        if body.is_empty() {
            self.toast("No transcript yet.");
            return;
        }
        let name = format!("{}.txt", session.title.replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], ""));
        if let Some(path) = rfd::FileDialog::new().set_file_name(&name).add_filter("Text", &["txt"]).save_file() {
            if std::fs::write(&path, format!("{}\n\n{body}\n", session.title)).is_err() {
                self.toast("Could not save the transcript.");
            }
        }
    }

    fn selection_text(&self) -> String {
        let Some(session) = self.session() else { return String::new() };
        transcript_text(session, self.sel_start, self.sel_end, self.max_speakers)
    }

    fn log_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("LOG").color(DIM).size(11.0));
            if ui.button("Close").clicked() {
                self.log_open = false;
            }
        });
        ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
            for line in &self.logs {
                ui.label(RichText::new(line).color(MUTED).size(12.0).monospace());
            }
        });
    }
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    Frame::NONE
        .fill(BG_CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn primary(text: &str) -> Button<'_> {
    Button::new(RichText::new(text).strong()).fill(VIOLET).stroke(Stroke::new(1.0, Color32::from_rgb(177, 124, 255)))
}

fn pill(ui: &mut egui::Ui, engine: &EngineInfo) {
    let (label, color) = if engine.state == "ready" {
        ("Nemotron", GOOD)
    } else if engine.state == "error" || engine.state == "missing" {
        ("Engine", BAD)
    } else {
        ("Loading", WARN)
    };
    ui.label(RichText::new(format!("● {label}")).color(color));
    let gpu = short_gpu(&engine.gpu);
    ui.label(RichText::new(format!("● {gpu}")).color(if engine.gpu.is_empty() { DIM } else { GOOD }));
}

fn write_transcript_page(root: &Path, session: &Session, path: &Path) -> Result<(), String> {
    let mut turns = session.turns.clone();
    turns.retain(|turn| counted_speaker(turn.speaker, session.max_speakers));
    turns.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    let mut speakers: Vec<u8> = turns.iter().map(|turn| turn.speaker).collect();
    speakers.sort_unstable();
    speakers.dedup();
    let mut html = String::new();
    html.push_str("<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>");
    html.push_str(&html_escape(&session.title));
    html.push_str("</title><style>");
    html.push_str("body{margin:0;background:#080b18;color:#f3f1ff;font:16px/1.45 'Segoe UI',sans-serif}");
    html.push_str("main{max-width:820px;margin:0 auto;padding:32px 20px 64px}");
    html.push_str("h1{font-size:28px;margin:0 0 8px} .sub{color:#6e7494;margin:0 0 28px}");
    html.push_str(".people{display:flex;flex-wrap:wrap;gap:12px;margin:0 0 28px}");
    html.push_str(".person{display:flex;align-items:center;gap:8px;color:#a5a9c3}");
    html.push_str("img, .dot{width:42px;height:42px;border-radius:21px;object-fit:cover;flex:0 0 auto}");
    html.push_str(".line{display:grid;grid-template-columns:42px 1fr;gap:12px;padding:12px 0;border-top:1px solid #34375a}");
    html.push_str(".who{font-weight:700} .when{float:right;color:#6e7494;font-size:12px;font-weight:400}");
    html.push_str("p{margin:4px 0 0}");
    html.push_str("</style></head><body><main><h1>");
    html.push_str(&html_escape(&session.title));
    html.push_str("</h1><p class=\"sub\">");
    html.push_str(&html_escape(&format!("{} lines", turns.len())));
    html.push_str("</p><div class=\"people\">");
    for speaker in &speakers {
        html.push_str("<div class=\"person\">");
        html.push_str(&portrait_html(root, &session.id, *speaker));
        html.push_str("<span style=\"color:");
        html.push_str(&css_color(*speaker));
        html.push_str("\">");
        html.push_str(&html_escape(&speaker_label(session, *speaker)));
        html.push_str("</span></div>");
    }
    html.push_str("</div>");
    for turn in &turns {
        html.push_str("<article class=\"line\">");
        html.push_str(&portrait_html(root, &session.id, turn.speaker));
        html.push_str("<div><div class=\"who\" style=\"color:");
        html.push_str(&css_color(turn.speaker));
        html.push_str("\">");
        html.push_str(&html_escape(&speaker_label(session, turn.speaker)));
        html.push_str("<span class=\"when\">");
        html.push_str(&html_escape(&stamp(turn.start)));
        html.push_str("</span></div><p>");
        html.push_str(&html_escape(&turn.text));
        html.push_str("</p></div></article>");
    }
    html.push_str("</main></body></html>");
    std::fs::write(path, html).map_err(|error| error.to_string())
}

fn portrait_html(root: &Path, session_id: &str, speaker: u8) -> String {
    let path = root.join("sessions").join(session_id).join("portraits").join(format!("{speaker}.png"));
    if let Ok(bytes) = std::fs::read(path) {
        let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
        return format!("<img alt=\"\" src=\"data:image/png;base64,{encoded}\">");
    }
    format!("<span class=\"dot\" style=\"background:{}\"></span>", css_color(speaker))
}

fn speaker_label(session: &Session, speaker: u8) -> String {
    session.speakers.get(&speaker.to_string()).map(|info| info.name.clone()).unwrap_or_else(|| default_speaker_name(speaker))
}

fn css_color(speaker: u8) -> String {
    let color = voice_color(speaker);
    format!("rgb({}, {}, {})", color.r(), color.g(), color.b())
}

fn html_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn voice_color(speaker: u8) -> Color32 {
    match speaker {
        ME => Color32::from_rgb(255, 244, 214),
        OVERLAP_SPEAKER => Color32::from_rgb(255, 186, 120),
        UNASSIGNED_SPEAKER => Color32::from_rgb(150, 156, 176),
        _ => COLORS[speaker as usize % COLORS.len()],
    }
}

fn counted_speaker(speaker: u8, max_speakers: u8) -> bool {
    matches!(speaker, ME | OVERLAP_SPEAKER | UNASSIGNED_SPEAKER) || speaker < max_speakers
}

fn shown_in_wave(speaker: u8, max_speakers: u8) -> bool {
    matches!(speaker, OVERLAP_SPEAKER | UNASSIGNED_SPEAKER) || speaker < max_speakers
}

fn paint_wave(ui: &egui::Ui, rect: Rect, peaks: &[f32], segments: &[Segment], max_speakers: u8, start: f64, end: f64, position: f64, duration: f64) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(8), FIELD);
    painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);
    let mid = rect.center().y;
    let mut bands: Vec<&Segment> = segments.iter().filter(|segment| shown_in_wave(segment.speaker, max_speakers) && segment.end > segment.start).collect();
    bands.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    if duration > 0.0 {
        for segment in &bands {
            let x0 = rect.left() + (segment.start / duration).clamp(0.0, 1.0) as f32 * rect.width();
            let x1 = rect.left() + (segment.end / duration).clamp(0.0, 1.0) as f32 * rect.width();
            let band = Rect::from_min_max(Pos2::new(x0, rect.top() + 8.0), Pos2::new(x1.max(x0 + 2.0), rect.bottom() - 8.0));
            painter.rect_filled(band, 2.0, voice_color(segment.speaker).gamma_multiply(0.35));
        }
    }
    if peaks.is_empty() {
        painter.hline(rect.x_range(), mid, Stroke::new(1.0, Color32::from_rgb(40, 48, 78)));
    } else {
        let peak_max = peaks.iter().copied().fold(0.0f32, f32::max).max(0.05);
        let slot = rect.width() / peaks.len() as f32;
        let bar_w = (slot * 0.78).clamp(1.0, 5.0);
        let mut cursor = 0usize;
        for (index, peak) in peaks.iter().enumerate() {
            let time = if duration > 0.0 { index as f64 / peaks.len() as f64 * duration } else { 0.0 };
            while cursor < bands.len() && bands[cursor].end <= time {
                cursor += 1;
            }
            let mut speaker = None;
            let mut probe = cursor;
            while probe < bands.len() && bands[probe].start <= time {
                if time < bands[probe].end {
                    speaker = Some(bands[probe].speaker);
                    break;
                }
                probe += 1;
            }
            let color = speaker.map(voice_color).unwrap_or(Color32::from_rgb(150, 164, 196));
            let height = ((peak / peak_max) * rect.height() * 0.84).max(1.5);
            let x = rect.left() + index as f32 * slot + (slot - bar_w) * 0.5;
            painter.rect_filled(Rect::from_min_size(Pos2::new(x, mid - height / 2.0), Vec2::new(bar_w, height)), 1.0, color);
        }
    }
    if duration > 0.0 {
        let partial = start > 0.2 || end < duration - 0.2;
        if partial {
            let left = rect.left() + (start / duration).clamp(0.0, 1.0) as f32 * rect.width();
            let right = rect.left() + (end / duration).clamp(0.0, 1.0) as f32 * rect.width();
            painter.vline(left, rect.y_range(), Stroke::new(1.0, VIOLET));
            painter.vline(right, rect.y_range(), Stroke::new(1.0, VIOLET));
        }
        let head = rect.left() + (position / duration).clamp(0.0, 1.0) as f32 * rect.width();
        painter.vline(head, rect.y_range(), Stroke::new(1.5, CYAN));
    }
}

fn time_at(rect: Rect, pos: Pos2, duration: f64) -> f64 {
    if duration <= 0.0 || rect.width() <= 0.0 {
        return 0.0;
    }
    ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * duration
}

fn stamp(value: f64) -> String {
    let time = value.max(0.0);
    let minutes = (time / 60.0).floor() as i64;
    format!("{minutes}:{:04.1}", time % 60.0)
}

fn short_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    if line.chars().count() > 180 {
        format!("{}…", line.chars().take(177).collect::<String>())
    } else {
        line.to_string()
    }
}

fn short_gpu(name: &str) -> String {
    let upper = name.to_ascii_uppercase();
    for token in ["RTX", "GTX"] {
        if let Some(index) = upper.find(token) {
            let rest = upper[index..].split_whitespace().take(2).collect::<Vec<_>>().join(" ");
            if !rest.is_empty() {
                return rest;
            }
        }
    }
    if name.is_empty() { "GPU".into() } else { name.to_string() }
}

fn transcript_text(session: &Session, start: f64, end: f64, max_speakers: u8) -> String {
    session
        .turns
        .iter()
        .filter(|turn| counted_speaker(turn.speaker, max_speakers) && turn.end >= start && turn.start <= end)
        .map(|turn| {
            let name = session.speakers.get(&turn.speaker.to_string()).map(|info| info.name.as_str()).unwrap_or("Speaker");
            format!("[{}] {name}: {}", stamp(turn.start), turn.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn file_title(path: &Path) -> String {
    path.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into())
}

fn prepare_import(root: &Path, id: &str, source: &Path, ext: &str) -> Result<Session, String> {
    let dir = root.join("sessions").join(id);
    let stored = dir.join(format!("source.{ext}"));
    std::fs::copy(source, &stored).map_err(|error| error.to_string())?;
    let wav = dir.join("audio.wav");
    media::extract_wav(&stored, &wav)?;
    let (duration, peaks) = media::peaks_from_wav(&wav, 2000).map_err(|error| error.to_string())?;
    let mut session = Session::new(&file_title(source), "file");
    session.id = id.to_string();
    session.media_file = format!("source.{ext}");
    session.visual = if video_ext(ext) { "video" } else { "audio" }.into();
    session.duration = duration;
    session.peaks = peaks;
    session.status = "working".into();
    session.save(root).map_err(|error| format!("Could not save this session ({error})"))?;
    Ok(session)
}

fn download_link(root: &Path, python: &Path, id: &str, url: &str, tx: &mpsc::Sender<AppMsg>) -> Result<Session, String> {
    let dir = root.join("sessions").join(id);
    let video_args = ["--newline", "--no-playlist", "-f", "bv*[height<=720]+ba/b", "--merge-output-format", "mp4", "--write-info-json", "-o", "source.%(ext)s"];
    if run_ytdlp(python, &dir, url, &video_args, id, tx).is_err() {
        clear_downloads(&dir);
        let audio_args = ["--newline", "--no-playlist", "-f", "ba/b", "-x", "--audio-format", "mp3", "--write-info-json", "-o", "source.%(ext)s"];
        run_ytdlp(python, &dir, url, &audio_args, id, tx)?;
    }
    let source = find_source(&dir).ok_or("The download finished without a media file.")?;
    let ext = source.extension().and_then(|ext| ext.to_str()).unwrap_or("mp4").to_ascii_lowercase();
    let wav = dir.join("audio.wav");
    media::extract_wav(&source, &wav)?;
    let (duration, peaks) = media::peaks_from_wav(&wav, 2000).map_err(|error| error.to_string())?;
    let mut session = Session::new("Opening link", "url");
    session.id = id.to_string();
    if let Some(title) = read_title(&dir) {
        session.title = title;
    } else {
        session.title = file_title(&source);
    }
    session.media_file = source.file_name().unwrap_or_default().to_string_lossy().to_string();
    session.visual = if video_ext(&ext) { "video" } else { "audio" }.into();
    session.duration = duration;
    session.peaks = peaks;
    session.save(root).map_err(|error| format!("Could not save this session ({error})"))?;
    Ok(session)
}

fn run_ytdlp(python: &Path, dir: &Path, url: &str, args: &[&str], id: &str, tx: &mpsc::Sender<AppMsg>) -> Result<(), String> {
    let mut cmd = Command::new(python);
    cmd.arg("-m").arg("yt_dlp").args(args).arg(url).current_dir(dir);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    hide_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|_| "yt-dlp is not installed in the speech engine yet.".to_string())?;
    let stdout = child.stdout.take().ok_or("yt-dlp produced no output.")?;
    let stderr = child.stderr.take();
    let log_tx = tx.clone();
    let stderr_thread = thread::spawn(move || {
        let mut collected = String::new();
        if let Some(stderr) = stderr {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if collected.len() < 4000 {
                    collected.push_str(line.trim());
                    collected.push('\n');
                }
                let _ = log_tx.send(AppMsg::Log(line));
            }
        }
        collected
    });
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let _ = tx.send(AppMsg::Log(line.clone()));
        if let Some(percent) = percent_of(&line) {
            let _ = tx.send(AppMsg::Note { id: id.to_string(), message: format!("Downloading {percent}%") });
        }
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    let detail = stderr_thread.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else if detail.trim().is_empty() {
        Err("Could not download that link.".into())
    } else {
        Err(detail.trim().to_string())
    }
}

fn percent_of(line: &str) -> Option<String> {
    let (head, _) = line.split_once('%')?;
    let start = head.rfind(|ch: char| !ch.is_ascii_digit() && ch != '.')?;
    let number = head[start + 1..].trim();
    if number.is_empty() { None } else { Some(number.to_string()) }
}

fn clear_downloads(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("source") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn find_source(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        let name = path.file_name()?.to_string_lossy().to_string();
        (name.starts_with("source.") && !name.ends_with(".json") && !name.ends_with(".part") && !name.ends_with(".ytdl") && path.is_file()).then_some(path)
    })
}

fn read_title(dir: &Path) -> Option<String> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let name = path.file_name()?.to_string_lossy().to_string();
        if !name.ends_with("info.json") {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        if let Some(title) = value.get("title").and_then(|item| item.as_str()) {
            return Some(clean_title(title));
        }
    }
    None
}

fn spawn_engine(root: &Path, tx: mpsc::Sender<AppMsg>) -> mpsc::Sender<String> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<String>();
    let python = python_executable(root);
    let root = root.to_path_buf();
    thread::spawn(move || {
        if !python.exists() {
            let _ = tx.send(AppMsg::Engine(EngineInfo {
                state: "missing".into(),
                detail: "Run start.ps1 once so the local speech engine can install.".into(),
                gpu: String::new(),
            }));
            return;
        }
        let mut cmd = Command::new(&python);
        cmd.arg("-u").arg(root.join("engine").join("engine.py")).current_dir(&root);
        cmd.env("PYTHONUNBUFFERED", "1").env("HF_HUB_DISABLE_PROGRESS_BARS", "1").env("TQDM_DISABLE", "1");
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        hide_window(&mut cmd);
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = tx.send(AppMsg::Failed { job: String::new(), message: format!("Could not start the speech engine ({error}).") });
                return;
            }
        };
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        let mut stdin = child.stdin.take().expect("stdin");
        let log_tx = tx.clone();
        let log_path = root.join("sessions").join("engine.log");
        thread::spawn(move || {
            let _ = std::fs::create_dir_all(log_path.parent().unwrap());
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                let _ = log_tx.send(AppMsg::Log(line.clone()));
                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
                    let _ = writeln!(file, "{line}");
                }
            }
        });
        let writer_rx = cmd_rx;
        thread::spawn(move || {
            while let Ok(line) = writer_rx.recv() {
                if writeln!(stdin, "{line}").is_err() || stdin.flush().is_err() {
                    break;
                }
            }
            let _ = child.kill();
        });
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(message) = parse_engine_line(&line) {
                if tx.send(message).is_err() {
                    break;
                }
            }
        }
        let _ = tx.send(AppMsg::Engine(EngineInfo { state: "error".into(), detail: "The speech engine stopped.".into(), gpu: String::new() }));
    });
    cmd_tx
}

fn parse_engine_line(line: &str) -> Option<AppMsg> {
    let value: serde_json::Value = serde_json::from_str(line.get(line.find('{')?..)?).ok()?;
    let event = value.get("event").and_then(|item| item.as_str()).unwrap_or("");
    let job = value.get("job").and_then(|item| item.as_str()).unwrap_or("").to_string();
    let message = value.get("message").and_then(|item| item.as_str()).unwrap_or("").to_string();
    match event {
        "ready" => Some(AppMsg::Engine(EngineInfo {
            state: "ready".into(),
            gpu: value.get("gpu").and_then(|item| item.as_str()).unwrap_or("").into(),
            detail: "Nemotron 3 ready".into(),
        })),
        "status" => Some(AppMsg::Status { job, message }),
        "result" => Some(AppMsg::Result {
            job,
            segments: serde_json::from_value(value.get("segments")?.clone()).unwrap_or_default(),
            turns: serde_json::from_value(value.get("turns")?.clone()).unwrap_or_default(),
            message,
            start: value.get("start").and_then(|item| item.as_f64()),
            end: value.get("end").and_then(|item| item.as_f64()),
        }),
        "live" => Some(AppMsg::Live {
            job,
            segments: serde_json::from_value(value.get("segments")?.clone()).unwrap_or_default(),
            turns: serde_json::from_value(value.get("turns")?.clone()).unwrap_or_default(),
        }),
        "error" => Some(AppMsg::Failed { job, message: if message.is_empty() { "The speech engine hit an error.".into() } else { message } }),
        _ => None,
    }
}

fn open_player(path: &Path, at: f64) -> Result<Player, String> {
    let (stream, handle) = rodio::OutputStream::try_default().map_err(|error| error.to_string())?;
    let sink = rodio::Sink::try_new(&handle).map_err(|error| error.to_string())?;
    if let Ok(source) = PcmSource::open(path, at) {
        sink.append(source);
    } else {
        let file = BufReader::new(File::open(path).map_err(|error| error.to_string())?);
        let source = rodio::Decoder::new(file).map_err(|error| error.to_string())?;
        sink.append(source.skip_duration(Duration::from_secs_f64(at.max(0.0))));
    }
    sink.play();
    Ok(Player { _stream: stream, sink })
}

impl VideoPump {
    fn start(path: &Path, at: f64) -> Option<Self> {
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-hide_banner").arg("-loglevel").arg("error").arg("-ss").arg(format!("{:.3}", at.max(0.0))).arg("-i").arg(path);
        cmd.args(["-an", "-vf", "scale=480:270:force_original_aspect_ratio=decrease,pad=480:270:(ow-iw)/2:(oh-ih)/2:black", "-r", "10", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]);
        cmd.stdout(Stdio::piped()).stderr(Stdio::null());
        hide_window(&mut cmd);
        let mut child = cmd.spawn().ok()?;
        let mut stdout = child.stdout.take()?;
        let latest = Arc::new(Mutex::new(None));
        let slot = latest.clone();
        thread::spawn(move || {
            let frame = 480 * 270 * 4;
            let mut buffer = vec![0u8; frame];
            let started = Instant::now();
            let mut index = 0u64;
            loop {
                let mut filled = 0;
                while filled < frame {
                    match stdout.read(&mut buffer[filled..]) {
                        Ok(0) | Err(_) => return,
                        Ok(count) => filled += count,
                    }
                }
                let due = started + Duration::from_millis(index.saturating_mul(100));
                index += 1;
                let now = Instant::now();
                if let Some(wait) = due.checked_duration_since(now) {
                    thread::sleep(wait);
                } else if now.duration_since(due) > Duration::from_millis(100) {
                    continue;
                }
                if let Ok(mut guard) = slot.lock() {
                    *guard = Some(buffer.clone());
                }
            }
        });
        Some(Self { child, latest })
    }
}

fn mix_wavs(desktop: &Path, mic: &Path, output: &Path) -> Result<(), String> {
    let desktop_ok = desktop.exists() && desktop.metadata().map(|meta| meta.len() > 44).unwrap_or(false);
    let mic_ok = mic.exists() && mic.metadata().map(|meta| meta.len() > 44).unwrap_or(false);
    if desktop_ok && mic_ok {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(desktop)
            .arg("-i")
            .arg(mic)
            .args(["-filter_complex", "amix=inputs=2:duration=longest:normalize=0", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
            .arg(output);
        hide_window(&mut cmd);
        let status = cmd.status().map_err(|error| error.to_string())?;
        if status.success() {
            return Ok(());
        }
    }
    let source = if desktop_ok { desktop } else if mic_ok { mic } else { return Err("No audio was captured.".into()) };
    std::fs::copy(source, output).map_err(|error| error.to_string())?;
    Ok(())
}

fn overlaps_time(start: f64, end: f64, range_start: f64, range_end: f64) -> bool {
    end > range_start && start < range_end
}

fn merge_selection(session: &mut Session, segments: Vec<Segment>, turns: Vec<Turn>, start: f64, end: f64) {
    let old: Vec<Segment> = session
        .segments
        .iter()
        .filter(|segment| segment.speaker < ME && overlaps_time(segment.start, segment.end, start, end))
        .cloned()
        .collect();
    session.segments.retain(|segment| segment.speaker == ME || !overlaps_time(segment.start, segment.end, start, end));
    session.turns.retain(|turn| turn.speaker == ME || !overlaps_time(turn.start, turn.end, start, end));
    let mut incoming = Vec::new();
    for speaker in segments.iter().map(|segment| segment.speaker).chain(turns.iter().map(|turn| turn.speaker)) {
        if speaker < ME && !incoming.contains(&speaker) {
            incoming.push(speaker);
        }
    }
    let mut map = HashMap::<u8, u8>::new();
    for speaker in incoming {
        let mut best: Option<(u8, f64)> = None;
        for segment in segments.iter().filter(|segment| segment.speaker == speaker) {
            for previous in &old {
                let overlap = (segment.end.min(previous.end) - segment.start.max(previous.start)).max(0.0);
                if overlap > best.map(|(_, value)| value).unwrap_or(0.0) {
                    best = Some((previous.speaker, overlap));
                }
            }
        }
        if let Some((id, overlap)) = best {
            if overlap >= 0.3 {
                map.insert(speaker, id);
                continue;
            }
        }
        let mut used = std::collections::HashSet::new();
        for id in session
            .segments
            .iter()
            .map(|segment| segment.speaker)
            .chain(session.turns.iter().map(|turn| turn.speaker))
            .chain(map.values().copied())
        {
            if id < ME {
                used.insert(id);
            }
        }
        if let Some(id) = (0..ME).find(|id| !used.contains(id)) {
            map.insert(speaker, id);
        } else if let Some((id, _)) = best {
            map.insert(speaker, id);
        }
    }
    for segment in segments {
        if segment.speaker >= ME {
            continue;
        }
        let speaker = map.get(&segment.speaker).copied().unwrap_or(segment.speaker);
        session.segments.push(Segment { speaker, start: segment.start, end: segment.end });
    }
    for turn in turns {
        if turn.speaker >= ME {
            continue;
        }
        let speaker = map.get(&turn.speaker).copied().unwrap_or(turn.speaker);
        session.turns.push(Turn { speaker, start: turn.start, end: turn.end, text: turn.text });
    }
    session.segments.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    session.turns.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
}

impl CaptureHub {
    fn output(&self, kind: AudioKind) -> PcmOut {
        PcmOut {
            tx: self.tx.as_ref().expect("capture hub is open").clone(),
            queued: self.queued.clone(),
            dropped: self.dropped.clone(),
            kind,
        }
    }

    fn send_control(&self, packet: CapturePkt) {
        let Some(tx) = &self.tx else { return };
        self.queued.fetch_add(1, Ordering::Relaxed);
        if tx.send(packet).is_err() {
            self.queued.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

fn spawn_capture_hub(id: String, dir: PathBuf, party: media::LiveWav, mic: Option<media::LiveWav>, cmd: mpsc::Sender<String>) -> CaptureHub {
    let (tx, rx) = mpsc::channel();
    let meters = Arc::new(Mutex::new(CaptureMeters { party_peaks: Vec::new(), mic_peaks: Vec::new(), duration: 0.0, fault: None }));
    let queued = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicU64::new(0));
    let meters_thread = meters.clone();
    let queued_thread = queued.clone();
    let join = thread::spawn(move || run_capture_hub(id, dir, party, mic, cmd, rx, meters_thread, queued_thread));
    CaptureHub { tx: Some(tx), meters, queued, dropped, join: Some(join) }
}

fn run_capture_hub(
    id: String,
    dir: PathBuf,
    mut party: media::LiveWav,
    mut mic: Option<media::LiveWav>,
    cmd: mpsc::Sender<String>,
    rx: mpsc::Receiver<CapturePkt>,
    meters: Arc<Mutex<CaptureMeters>>,
    queued: Arc<AtomicUsize>,
) {
    let mut mic_on = mic.is_some();
    let mut last_meter = Instant::now();
    let mut last_send = Instant::now();
    let mut desktop_send = Vec::<i16>::new();
    let mut mic_send = Vec::<i16>::new();
    loop {
        let packet = if desktop_send.is_empty() && mic_send.is_empty() {
            match rx.recv() {
                Ok(packet) => packet,
                Err(_) => break,
            }
        } else {
            match rx.recv_timeout(Duration::from_millis(40)) {
                Ok(packet) => packet,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    flush_live_pcm(&cmd, &id, &mut desktop_send, &mut mic_send);
                    last_send = Instant::now();
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        };
        queued.fetch_sub(1, Ordering::Relaxed);
        match packet {
            CapturePkt::Desktop(pcm) => {
                if let Err(error) = party.push(&pcm) {
                    note_capture_fault(&meters, error.to_string());
                } else {
                    desktop_send.extend_from_slice(&pcm);
                }
            }
            CapturePkt::Mic(pcm) => {
                if mic_on {
                    if let Some(wav) = mic.as_mut() {
                        if let Err(error) = wav.push(&pcm) {
                            note_capture_fault(&meters, error.to_string());
                        } else {
                            mic_send.extend_from_slice(&pcm);
                        }
                    }
                }
            }
            CapturePkt::MicOn => {
                flush_live_pcm(&cmd, &id, &mut desktop_send, &mut mic_send);
                if mic.is_none() {
                    match media::LiveWav::create(&dir.join("mic.wav")) {
                        Ok(wav) => mic = Some(wav),
                        Err(error) => note_capture_fault(&meters, error.to_string()),
                    }
                }
                if let Some(wav) = mic.as_mut() {
                    let gap = party.samples().saturating_sub(wav.samples());
                    if gap > 0 {
                        if let Err(error) = wav.write_silence(gap) {
                            note_capture_fault(&meters, error.to_string());
                        } else {
                            let seconds = gap as f64 / 16000.0;
                            let _ = cmd.send(serde_json::json!({"cmd": "mic_pad", "job": id, "seconds": seconds}).to_string());
                        }
                    }
                }
                mic_on = mic.is_some();
                last_send = Instant::now();
            }
            CapturePkt::MicOff => {
                flush_live_pcm(&cmd, &id, &mut desktop_send, &mut mic_send);
                mic_on = false;
                last_send = Instant::now();
            }
        }
        if last_send.elapsed() >= Duration::from_millis(50) || desktop_send.len() + mic_send.len() >= 4000 {
            flush_live_pcm(&cmd, &id, &mut desktop_send, &mut mic_send);
            last_send = Instant::now();
        }
        if last_meter.elapsed() >= Duration::from_millis(80) {
            publish_capture_meters(&party, mic.as_ref(), &meters);
            last_meter = Instant::now();
        }
    }
    flush_live_pcm(&cmd, &id, &mut desktop_send, &mut mic_send);
    publish_capture_meters(&party, mic.as_ref(), &meters);
}

fn flush_live_pcm(cmd: &mpsc::Sender<String>, id: &str, desktop: &mut Vec<i16>, mic: &mut Vec<i16>) {
    if !desktop.is_empty() {
        send_live_pcm(cmd, id, "desktop", desktop);
        desktop.clear();
    }
    if !mic.is_empty() {
        send_live_pcm(cmd, id, "mic", mic);
        mic.clear();
    }
}

fn publish_capture_meters(party: &media::LiveWav, mic: Option<&media::LiveWav>, meters: &Mutex<CaptureMeters>) {
    if let Ok(mut guard) = meters.lock() {
        guard.party_peaks = party.snapshot();
        guard.mic_peaks = mic.map(|wav| wav.snapshot()).unwrap_or_default();
        let mic_duration = mic.map(|wav| wav.duration()).unwrap_or(0.0);
        guard.duration = party.duration().max(mic_duration);
    }
}

fn note_capture_fault(meters: &Mutex<CaptureMeters>, message: String) {
    if let Ok(mut guard) = meters.lock() {
        if guard.fault.is_none() {
            guard.fault = Some(message);
        }
    }
}

fn send_live_pcm(cmd: &mpsc::Sender<String>, id: &str, source: &str, pcm: &[i16]) {
    let bytes: Vec<u8> = pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect();
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
    let _ = cmd.send(serde_json::json!({"cmd": "live_pcm", "job": id, "source": source, "pcm": encoded}).to_string());
}

struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn lowpass(rate: f32, cutoff: f32) -> Self {
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let w0 = 2.0 * std::f32::consts::PI * (cutoff / rate.max(1.0));
        let cos = w0.cos();
        let sin = w0.sin();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - cos) * 0.5) / a0,
            b1: (1.0 - cos) / a0,
            b2: ((1.0 - cos) * 0.5) / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn process(&mut self, sample: f32) -> f32 {
        let filtered = self.b0 * sample + self.z1;
        self.z1 = self.b1 * sample - self.a1 * filtered + self.z2;
        self.z2 = self.b2 * sample - self.a2 * filtered;
        filtered
    }
}

struct ResampleState {
    pending: Vec<f32>,
    phase: f64,
    low1: Biquad,
    low2: Biquad,
    use_filter: bool,
}

impl ResampleState {
    fn new(rate: u32) -> Self {
        Self {
            pending: Vec::new(),
            phase: 0.0,
            low1: Biquad::lowpass(rate as f32, 7000.0),
            low2: Biquad::lowpass(rate as f32, 7000.0),
            use_filter: rate > 18_000,
        }
    }
}

struct PcmSource {
    file: File,
    rate: u32,
    remaining: u64,
    total: Duration,
    buf: Vec<i16>,
    index: usize,
    len: usize,
}

impl PcmSource {
    fn open(path: &Path, at: f64) -> std::io::Result<Self> {
        let (file, rate, remaining) = media::open_wav_pcm(path, at)?;
        let total = Duration::from_secs_f64(remaining as f64 / rate.max(1) as f64);
        Ok(Self { file, rate, remaining, total, buf: vec![0; 2048], index: 0, len: 0 })
    }
}

impl Iterator for PcmSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.index >= self.len {
            if self.remaining == 0 {
                return None;
            }
            let want = (self.buf.len() as u64).min(self.remaining) as usize;
            let mut bytes = vec![0u8; want * 2];
            if self.file.read_exact(&mut bytes).is_err() {
                self.remaining = 0;
                return None;
            }
            for (offset, chunk) in bytes.chunks_exact(2).enumerate() {
                self.buf[offset] = i16::from_le_bytes([chunk[0], chunk[1]]);
            }
            self.len = want;
            self.index = 0;
            self.remaining -= want as u64;
        }
        let sample = self.buf[self.index] as f32 / 32768.0;
        self.index += 1;
        Some(sample)
    }
}

impl Source for PcmSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        1
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(self.total)
    }
}

fn start_desktop(device_id: String, name: String, output: PcmOut) -> Result<Desktop, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (err_tx, err_rx) = mpsc::channel();
    let running = stop.clone();
    let join = thread::spawn(move || {
        let _ = err_tx.send(desktop_loop(output, running, device_id));
    });
    thread::sleep(Duration::from_millis(200));
    match err_rx.try_recv() {
        Ok(Err(error)) => Err(error),
        Ok(Ok(())) => Err("Desktop audio stopped immediately.".into()),
        Err(_) => Ok(Desktop { stop, join: Some(join), name }),
    }
}

fn list_output_devices() -> Vec<(String, String)> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = wasapi::initialize_mta();
        let mut found = Vec::new();
        if let Ok(enumerator) = wasapi::DeviceEnumerator::new() {
            if let Ok(collection) = enumerator.get_device_collection(&wasapi::Direction::Render) {
                if let Ok(count) = collection.get_nbr_devices() {
                    for index in 0..count {
                        let Ok(device) = collection.get_device_at_index(index) else { continue };
                        let Ok(id) = device.get_id() else { continue };
                        let name = device.get_friendlyname().unwrap_or_else(|_| format!("Output {}", index + 1));
                        if !id.is_empty() {
                            found.push((id, name));
                        }
                    }
                }
            }
        }
        let _ = tx.send(found);
    });
    rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default()
}

fn desktop_loop(output: PcmOut, stop: Arc<AtomicBool>, device_id: String) -> Result<(), String> {
    let _ = wasapi::initialize_mta();
    let enumerator = wasapi::DeviceEnumerator::new().map_err(|error| error.to_string())?;
    let device = if device_id.is_empty() {
        enumerator.get_default_device(&wasapi::Direction::Render).map_err(|error| error.to_string())?
    } else {
        enumerator.get_device(&device_id).map_err(|error| error.to_string())?
    };
    let mut client = device.get_iaudioclient().map_err(|error| error.to_string())?;
    let format = wasapi::WaveFormat::new(32, 32, &wasapi::SampleType::Float, 48000, 2, None);
    let (_default_period, min_period) = client.get_device_period().map_err(|error| error.to_string())?;
    let mode = wasapi::StreamMode::PollingShared { autoconvert: true, buffer_duration_hns: min_period.max(200_000) };
    client.initialize_client(&format, &wasapi::Direction::Capture, &mode).map_err(|error| error.to_string())?;
    let capture = client.get_audiocaptureclient().map_err(|error| error.to_string())?;
    client.start_stream().map_err(|error| error.to_string())?;
    let mut bytes = vec![0u8; 48000 * 8];
    let mut resample = ResampleState::new(48000);
    while !stop.load(Ordering::Relaxed) {
        let packet = capture.get_next_packet_size().ok().flatten().unwrap_or(0);
        if packet == 0 {
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        let need = packet as usize * 8;
        if bytes.len() < need {
            bytes.resize(need, 0);
        }
        let Ok((frames, _info)) = capture.read_from_device(&mut bytes[..need]) else {
            thread::sleep(Duration::from_millis(5));
            continue;
        };
        let mut mono = Vec::with_capacity(frames as usize);
        for index in 0..frames as usize {
            let base = index * 8;
            if base + 8 > bytes.len() {
                break;
            }
            let left = f32::from_le_bytes(bytes[base..base + 4].try_into().unwrap_or([0; 4]));
            let right = f32::from_le_bytes(bytes[base + 4..base + 8].try_into().unwrap_or([0; 4]));
            mono.push((left + right) * 0.5);
        }
        emit_pcm(&output, &mut resample, &mono, 48000);
    }
    let _ = client.stop_stream();
    Ok(())
}

fn start_mic(output: PcmOut) -> Result<Mic, String> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or("No microphone was found.")?;
    let name = device.name().unwrap_or_else(|_| "Microphone".into());
    let supported = device.default_input_config().map_err(|error| error.to_string())?;
    let rate = supported.sample_rate().0;
    let channels = supported.channels().max(1) as usize;
    let config: cpal::StreamConfig = supported.clone().into();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            let mut resample = ResampleState::new(rate);
            let output = output.clone();
            device
                .build_input_stream(
                    &config,
                    move |data: &[f32], _| emit_pcm(&output, &mut resample, &mono_f32(data, channels), rate),
                    |error| eprintln!("microphone: {error}"),
                    None,
                )
                .map_err(|error| error.to_string())?
        }
        cpal::SampleFormat::I16 => {
            let mut resample = ResampleState::new(rate);
            let output = output.clone();
            device
                .build_input_stream(
                    &config,
                    move |data: &[i16], _| emit_pcm(&output, &mut resample, &mono_i16(data, channels), rate),
                    |error| eprintln!("microphone: {error}"),
                    None,
                )
                .map_err(|error| error.to_string())?
        }
        other => return Err(format!("This microphone format is not supported ({other}).")),
    };
    stream.play().map_err(|error| error.to_string())?;
    Ok(Mic { _stream: stream, name })
}

fn mono_f32(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect()
}

fn mono_i16(data: &[i16], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.iter().map(|sample| *sample as f32 / 32768.0).collect();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().map(|sample| *sample as f32 / 32768.0).sum::<f32>() / channels as f32)
        .collect()
}

fn emit_pcm(output: &PcmOut, state: &mut ResampleState, mono: &[f32], rate: u32) {
    if state.use_filter {
        for sample in mono {
            let filtered = state.low2.process(state.low1.process(*sample));
            state.pending.push(filtered);
        }
    } else {
        state.pending.extend_from_slice(mono);
    }
    let step = rate.max(1) as f64 / 16000.0;
    let mut pcm = Vec::new();
    while state.phase + 1.0 < state.pending.len() as f64 {
        let index = state.phase as usize;
        let mix = (state.phase - index as f64) as f32;
        let sample = state.pending[index] * (1.0 - mix) + state.pending[index + 1] * mix;
        pcm.push((sample.clamp(-1.0, 1.0) * 32767.0) as i16);
        state.phase += step;
    }
    let drop_count = (state.phase as usize).min(state.pending.len());
    if drop_count > 0 {
        state.pending.drain(..drop_count);
        state.phase -= drop_count as f64;
    }
    if pcm.is_empty() {
        return;
    }
    let depth = output.queued.fetch_add(1, Ordering::Relaxed);
    if depth >= QUEUE_CAP {
        output.queued.fetch_sub(1, Ordering::Relaxed);
        output.dropped.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let packet = match output.kind {
        AudioKind::Desktop => CapturePkt::Desktop(pcm),
        AudioKind::Mic => CapturePkt::Mic(pcm),
    };
    if output.tx.send(packet).is_err() {
        output.queued.fetch_sub(1, Ordering::Relaxed);
        output.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

fn load_capture_settings(root: &Path) -> (bool, String) {
    let path = root.join("sessions").join("studio-settings.json");
    let value = std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let mic = value.as_ref().and_then(|item| item.get("mic_enabled").and_then(|flag| flag.as_bool())).unwrap_or(false);
    let device = value.as_ref().and_then(|item| item.get("output_device_id").and_then(|id| id.as_str())).unwrap_or("").to_string();
    (mic, device)
}

fn save_capture_settings(root: &Path, enabled: bool, device_id: &str) {
    let path = root.join("sessions").join("studio-settings.json");
    let _ = std::fs::create_dir_all(root.join("sessions"));
    let _ = std::fs::write(path, serde_json::json!({"mic_enabled": enabled, "output_device_id": device_id}).to_string());
}

fn python_executable(root: &Path) -> PathBuf {
    let scripts = root.join(".engine").join("Scripts");
    let windowless = scripts.join("pythonw.exe");
    if windowless.exists() { windowless } else { scripts.join("python.exe") }
}

fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
}

fn project_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if cwd.join("engine").join("engine.py").exists() {
        return cwd;
    }
    let mut dir = std::env::current_exe().unwrap_or(cwd.clone());
    for _ in 0..5 {
        if dir.join("engine").join("engine.py").exists() {
            return dir;
        }
        if !dir.pop() {
            break;
        }
    }
    cwd
}

fn style_ui(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG0;
    visuals.window_fill = BG0;
    visuals.extreme_bg_color = FIELD;
    visuals.faint_bg_color = BG_CARD;
    visuals.code_bg_color = FIELD;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = VIOLET;
    visuals.hyperlink_color = CYAN;
    visuals.widgets.noninteractive.bg_fill = BG_CARD;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(32, 41, 71);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(84, 56, 139);
    visuals.widgets.active.bg_fill = VIOLET;
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    ctx.set_visuals(visuals);
}

fn claim_single_instance() -> bool {
    let name: Vec<u16> = "Local\\SpeakerStudio.Nemotron\0".encode_utf16().collect();
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
    if handle.is_null() {
        return true;
    }
    if unsafe { GetLastError() } == 183 {
        focus_existing();
        return false;
    }
    INSTANCE_LOCK.store(handle, std::sync::atomic::Ordering::Relaxed);
    true
}

static INSTANCE_LOCK: std::sync::atomic::AtomicPtr<core::ffi::c_void> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

fn focus_existing() {
    let title: Vec<u16> = "Speaker Studio\0".encode_utf16().collect();
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if !hwnd.is_null() {
            ShowWindow(hwnd, 9);
            SetForegroundWindow(hwnd);
        }
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(attributes: *mut core::ffi::c_void, initial_owner: i32, name: *const u16) -> *mut core::ffi::c_void;
    fn GetLastError() -> u32;
}

#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class: *const u16, window: *const u16) -> *mut core::ffi::c_void;
    fn SetForegroundWindow(hwnd: *mut core::ffi::c_void) -> i32;
    fn ShowWindow(hwnd: *mut core::ffi::c_void, command: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_keeps_outside_audio_and_remaps_speakers() {
        let mut session = Session::new("Range", "file");
        session.segments = vec![
            Segment { speaker: 3, start: 0.0, end: 5.0 },
            Segment { speaker: 1, start: 6.0, end: 9.0 },
            Segment { speaker: ME, start: 1.0, end: 2.0 },
        ];
        session.turns = vec![
            Turn { speaker: 3, start: 0.0, end: 5.0, text: "inside".into() },
            Turn { speaker: 1, start: 6.0, end: 9.0, text: "later".into() },
            Turn { speaker: ME, start: 1.0, end: 2.0, text: "me".into() },
        ];
        session.speakers.insert("3".into(), SpeakerInfo { name: "Cara".into() });
        merge_selection(
            &mut session,
            vec![Segment { speaker: 0, start: 0.2, end: 4.8 }],
            vec![Turn { speaker: 0, start: 0.2, end: 4.8, text: "replaced".into() }],
            0.0,
            5.0,
        );
        assert!(session.turns.iter().any(|turn| turn.speaker == 3 && turn.text == "replaced"));
        assert!(session.turns.iter().any(|turn| turn.speaker == 1 && turn.text == "later"));
        assert!(session.turns.iter().any(|turn| turn.speaker == ME && turn.text == "me"));
        assert!(!session.turns.iter().any(|turn| turn.text == "inside"));
        assert!(counted_speaker(ME, 8));
        assert!(counted_speaker(OVERLAP_SPEAKER, 8));
        assert!(counted_speaker(UNASSIGNED_SPEAKER, 8));
    }

    #[test]
    fn lowpass_cuts_frequencies_that_would_alias() {
        fn level(freq: f32) -> f32 {
            let mut first = Biquad::lowpass(48000.0, 7000.0);
            let mut second = Biquad::lowpass(48000.0, 7000.0);
            let mut energy = 0.0f32;
            let mut count = 0.0f32;
            for index in 0..48000 {
                let time = index as f32 / 48000.0;
                let sample = (2.0 * std::f32::consts::PI * freq * time).sin();
                let filtered = second.process(first.process(sample));
                if index > 8000 {
                    energy += filtered * filtered;
                    count += 1.0;
                }
            }
            (energy / count).sqrt()
        }
        let speech = level(1000.0);
        let alias = level(15000.0);
        assert!(speech > alias * 4.0, "speech {speech} alias {alias}");
    }
}
