use crate::library::Track;
use crate::platform;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

pub enum Cmd {
    Play { queue: Vec<Track>, index: usize },
    Toggle,
    Next,
    Prev,
    Seek(f32),
    Volume(f32),
    Shuffle,
    Repeat,
    Enqueue(Track),
    PlayNext(Track),
    Jump(usize),
    Remove(usize),
    Forget(String),
    Refresh(Track),
    Snapshot,
}

#[derive(Clone)]
pub struct Snapshot {
    pub current: Option<Track>,
    pub cover: Option<String>,
    pub playing: bool,
    pub queue: Vec<Track>,
    pub index: usize,
    pub shuffle: bool,
    pub repeat: u8,
    pub volume: f32,
}

pub enum Event {
    Changed(Snapshot),
    Tick { pos: f32, dur: f32 },
    Error(String),
    Rescan,
    FolderPicked,
    ImagePicked,
}

#[derive(Clone)]
pub struct PlayerHandle(Sender<Cmd>);

impl PlayerHandle {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.0.send(cmd);
    }
}

pub fn spawn(
    settings: crate::store::Settings,
    on_event: impl Fn(Event) + Send + 'static,
) -> PlayerHandle {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("player".into())
        .spawn(move || {
            let mut engine = Engine {
                device: None,
                sink: None,
                queue: vec![],
                original: vec![],
                index: 0,
                playing: false,
                shuffle: settings.shuffle,
                repeat: settings.repeat.min(2),
                volume: settings.volume.clamp(0.0, 1.0),
                duration: 0.0,
                cover: None,
                parked: None,
                #[cfg(target_os = "ios")]
                resume_interrupted: false,
                on_event: Box::new(on_event),
            };
            engine.run(rx);
        })
        .expect("player thread");
    PlayerHandle(tx)
}

struct Engine {
    device: Option<rodio::MixerDeviceSink>,
    sink: Option<rodio::Player>,
    queue: Vec<Track>,
    original: Vec<Track>,
    index: usize,
    playing: bool,
    shuffle: bool,
    repeat: u8,
    volume: f32,
    duration: f32,
    cover: Option<String>,
    /// Position kept while the audio output is released to another app.
    parked: Option<f32>,
    #[cfg(target_os = "ios")]
    resume_interrupted: bool,
    on_event: Box<dyn Fn(Event) + Send>,
}

impl Engine {
    fn run(&mut self, rx: Receiver<Cmd>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            while let Some(action) = platform::poll_action() {
                if let Some(pos) = platform::seek_seconds(action) {
                    self.seek(pos);
                    continue;
                }
                match action as i32 {
                    #[cfg(target_os = "ios")]
                    platform::INTERRUPTION_BEGAN => {
                        if self.device.is_some() {
                            self.resume_interrupted = self.playing;
                            self.park();
                        }
                    }
                    #[cfg(target_os = "ios")]
                    platform::INTERRUPTION_ENDED | platform::INTERRUPTION_MAY_RESUME => {
                        if std::mem::take(&mut self.resume_interrupted)
                            && action == i64::from(platform::INTERRUPTION_MAY_RESUME)
                            && !self.playing
                        {
                            self.resume_parked();
                        }
                    }
                    #[cfg(target_os = "ios")]
                    platform::ACTION_PAUSE | platform::ACTION_PARK if !self.playing => {
                        self.resume_interrupted = false;
                    }
                    platform::ACTION_TOGGLE => self.handle(Cmd::Toggle),
                    platform::ACTION_NEXT => self.handle(Cmd::Next),
                    platform::ACTION_PREV => self.handle(Cmd::Prev),
                    platform::ACTION_PAUSE if self.playing => self.handle(Cmd::Toggle),
                    platform::ACTION_PLAY if !self.playing => self.handle(Cmd::Toggle),
                    platform::ACTION_PARK if self.playing => self.park(),
                    platform::ACTION_RESCAN => (self.on_event)(Event::Rescan),
                    platform::ACTION_FOLDER => (self.on_event)(Event::FolderPicked),
                    platform::ACTION_IMAGE => (self.on_event)(Event::ImagePicked),
                    _ => {}
                }
            }
            if self.playing && self.sink.as_ref().is_some_and(|s| s.empty()) {
                self.finished();
            }
            if self.sink.is_some() {
                (self.on_event)(Event::Tick {
                    pos: self.position(),
                    dur: self.duration,
                });
            }
        }
    }

    fn position(&self) -> f32 {
        self.sink
            .as_ref()
            .map(|s| s.get_pos().as_secs_f32())
            .or(self.parked)
            .unwrap_or(0.0)
            .min(self.duration.max(0.0))
    }

    /// Pauses and closes the audio output, remembering where to resume.
    fn park(&mut self) {
        let pos = self.position();
        self.sink = None;
        self.device = None;
        self.parked = Some(pos);
        self.playing = false;
        self.changed();
    }

    fn resume_parked(&mut self) -> bool {
        let Some(pos) = self.parked.take() else {
            return false;
        };
        self.seek(pos);
        match &self.sink {
            Some(sink) => {
                sink.play();
                self.playing = true;
                self.changed();
                true
            }
            None => false,
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        #[cfg(target_os = "ios")]
        if matches!(
            cmd,
            Cmd::Toggle | Cmd::Play { .. } | Cmd::Jump(_) | Cmd::Next | Cmd::Prev | Cmd::Forget(_)
        ) {
            self.resume_interrupted = false;
        }
        match cmd {
            Cmd::Play { queue, index } => {
                self.original = queue.clone();
                self.queue = queue;
                self.index = index.min(self.queue.len().saturating_sub(1));
                if self.shuffle {
                    self.shuffle_rest();
                }
                self.start_playing(0);
            }
            Cmd::Toggle => match &self.sink {
                Some(sink) if self.playing => {
                    sink.pause();
                    self.playing = false;
                    self.changed();
                }
                Some(sink) => {
                    sink.play();
                    self.playing = true;
                    self.changed();
                }
                None if !self.queue.is_empty() => {
                    if !self.resume_parked() {
                        self.start_playing(0);
                    }
                }
                None => {}
            },
            Cmd::Next => self.skip(true),
            Cmd::Prev => {
                if self.position() > 3.0 || self.index == 0 && self.repeat == 0 {
                    self.seek(0.0);
                } else {
                    self.skip(false);
                }
            }
            Cmd::Seek(secs) => self.seek(secs),
            Cmd::Volume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                if let Some(s) = &self.sink {
                    s.set_volume(self.volume);
                }
                self.changed();
            }
            Cmd::Shuffle => {
                self.shuffle = !self.shuffle;
                if self.shuffle {
                    self.shuffle_rest();
                } else if let Some(cur) = self.queue.get(self.index).cloned() {
                    self.queue = self.original.clone();
                    self.index = self
                        .queue
                        .iter()
                        .position(|t| t.path == cur.path)
                        .unwrap_or(0);
                }
                self.changed();
            }
            Cmd::Repeat => {
                self.repeat = (self.repeat + 1) % 3;
                self.changed();
            }
            Cmd::Enqueue(track) => self.enqueue(track, false),
            Cmd::PlayNext(track) => self.enqueue(track, true),
            Cmd::Jump(i) if i < self.queue.len() => {
                self.index = i;
                self.start_playing(0);
            }
            Cmd::Remove(i) if i < self.queue.len() && i != self.index => {
                let removed = self.queue.remove(i);
                if let Some(pos) = self.original.iter().position(|t| t.path == removed.path) {
                    self.original.remove(pos);
                }
                if i < self.index {
                    self.index -= 1;
                }
                self.changed();
            }
            Cmd::Forget(path) => {
                let playing_it = self.queue.get(self.index).is_some_and(|t| t.path == path);
                if playing_it {
                    self.parked = None;
                    self.sink = None;
                    self.playing = false;
                    self.cover = None;
                    self.duration = 0.0;
                }
                let current = self.queue.get(self.index).map(|t| t.path.clone());
                self.queue.retain(|t| t.path != path);
                self.original.retain(|t| t.path != path);
                self.index = current
                    .and_then(|c| self.queue.iter().position(|t| t.path == c))
                    .unwrap_or(self.index.min(self.queue.len().saturating_sub(1)));
                if playing_it && !self.queue.is_empty() {
                    self.cover = cover_file(&self.queue[self.index].path);
                    self.duration = self.queue[self.index].duration_secs as f32;
                }
                self.changed();
            }
            Cmd::Refresh(track) => {
                for t in self.queue.iter_mut().chain(self.original.iter_mut()) {
                    if t.path == track.path {
                        *t = track.clone();
                    }
                }
                if self
                    .queue
                    .get(self.index)
                    .is_some_and(|t| t.path == track.path)
                {
                    self.cover = cover_file(&track.path);
                }
                self.changed();
            }
            Cmd::Snapshot => self.changed(),
            Cmd::Jump(_) | Cmd::Remove(_) => {}
        }
    }

    fn enqueue(&mut self, track: Track, next: bool) {
        let at = if self.queue.is_empty() {
            self.cover = cover_file(&track.path);
            self.duration = track.duration_secs as f32;
            0
        } else if next {
            self.index + 1
        } else {
            self.queue.len()
        };
        self.original.push(track.clone());
        self.queue.insert(at, track);
        self.changed();
    }

    fn shuffle_rest(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let current = self.queue.remove(self.index);
        fastrand::shuffle(&mut self.queue);
        self.queue.insert(0, current);
        self.index = 0;
    }

    fn seek(&mut self, secs: f32) {
        if self.sink.is_none() {
            // A queued (or finished) track can be scrubbed before pressing Play.
            let Some(track) = self.queue.get(self.index).cloned() else {
                return;
            };
            self.parked = None;
            match self.open(&track) {
                Ok((sink, duration)) => {
                    sink.pause();
                    self.sink = Some(sink);
                    self.duration = duration;
                }
                Err(e) => {
                    (self.on_event)(Event::Error(e));
                    return;
                }
            }
        }
        if let Some(sink) = &self.sink {
            if !secs.is_finite() || self.duration <= 0.0 {
                return;
            }
            let secs = secs.clamp(0.0, self.duration);
            if let Err(e) = sink.try_seek(Duration::from_secs_f32(secs)) {
                log::warn!("seek failed: {e}");
                (self.on_event)(Event::Error("No se pudo avanzar en este archivo".into()));
                self.publish_media();
                return;
            }
            self.publish_media();
            (self.on_event)(Event::Tick {
                pos: secs,
                dur: self.duration,
            });
        }
    }

    fn skip(&mut self, forward: bool) {
        if self.queue.is_empty() {
            return;
        }
        let len = self.queue.len();
        self.index = if forward {
            (self.index + 1) % len
        } else {
            (self.index + len - 1) % len
        };
        self.start_playing(0);
    }

    fn finished(&mut self) {
        let last = self.index + 1 >= self.queue.len();
        match self.repeat {
            2 => self.start_playing(0),
            _ if !last => {
                self.index += 1;
                self.start_playing(0);
            }
            1 => {
                self.index = 0;
                self.start_playing(0);
            }
            _ => {
                self.sink = None;
                self.playing = false;
                self.changed();
            }
        }
    }

    /// Starts the track at `self.index`, skipping unreadable files.
    fn start_playing(&mut self, failures: usize) {
        let Some(track) = self.queue.get(self.index).cloned() else {
            return;
        };
        self.sink = None;
        self.parked = None;
        match self.open(&track) {
            Ok((sink, dur)) => {
                self.sink = Some(sink);
                self.duration = dur;
                self.playing = true;
                self.cover = cover_file(&track.path);
                self.changed();
            }
            Err(e) => {
                (self.on_event)(Event::Error(format!(
                    "No se pudo reproducir \"{}\": {e}",
                    track.title
                )));
                if failures + 1 < self.queue.len() && self.index + 1 < self.queue.len() {
                    self.index += 1;
                    self.start_playing(failures + 1);
                } else {
                    self.playing = false;
                    self.changed();
                }
            }
        }
    }

    fn open(&mut self, track: &Track) -> Result<(rodio::Player, f32), String> {
        use rodio::Source;
        if self.device.is_none() {
            let mut device =
                rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
            device.log_on_drop(false);
            self.device = Some(device);
        }
        let file = std::fs::File::open(&track.path).map_err(|e| e.to_string())?;
        let decoder = rodio::Decoder::try_from(file).map_err(|e| e.to_string())?;
        let dur = decoder
            .total_duration()
            .map(|d| d.as_secs_f32())
            .filter(|d| *d > 0.0)
            .unwrap_or(track.duration_secs as f32);
        let sink = rodio::Player::connect_new(self.device.as_ref().unwrap().mixer());
        sink.set_volume(self.volume);
        sink.append(crate::eq::Equalizer::new(decoder));
        Ok((sink, dur))
    }

    fn publish_media(&self) {
        if let Some(t) = self.queue.get(self.index) {
            platform::now_playing(
                &t.title,
                &t.artist,
                self.playing,
                (self.position() * 1000.0) as i64,
                (self.duration * 1000.0) as i64,
            );
        } else {
            platform::now_playing("", "", false, 0, 0);
        }
    }

    fn changed(&self) {
        let current = self
            .queue
            .get(self.index)
            .cloned()
            .filter(|_| self.sink.is_some() || !self.queue.is_empty());
        self.publish_media();
        if self.sink.is_none() {
            (self.on_event)(Event::Tick {
                pos: self.position(),
                dur: self.duration,
            });
        }
        crate::store::save_settings(&crate::store::Settings {
            volume: self.volume,
            shuffle: self.shuffle,
            repeat: self.repeat,
        });
        (self.on_event)(Event::Changed(Snapshot {
            current,
            cover: self.cover.clone(),
            playing: self.playing,
            queue: self.queue.clone(),
            index: self.index,
            shuffle: self.shuffle,
            repeat: self.repeat,
            volume: self.volume,
        }));
    }
}

fn cover_cache(path: &str, ext: &str) -> std::path::PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    crate::paths::get()
        .cache_dir
        .join("covers")
        .join(format!("{:x}.{ext}", h.finish()))
}

/// Drops the cached cover so a newly written one is shown.
pub fn forget_cover(path: &str) {
    for ext in ["jpg", "png"] {
        let _ = std::fs::remove_file(cover_cache(path, ext));
    }
}

/// Extracts the embedded cover to the cache so the UI can load it by path.
pub(crate) fn cached_cover(path: &str) -> Option<std::path::PathBuf> {
    let source_time = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    for ext in ["jpg", "png"] {
        let f = cover_cache(path, ext);
        if let Ok(time) = std::fs::metadata(&f).and_then(|m| m.modified()) {
            if source_time.is_none_or(|t| time >= t) {
                return Some(f);
            }
        }
    }
    None
}

fn cover_file(path: &str) -> Option<String> {
    if let Some(f) = cached_cover(path) {
        return Some(f.to_string_lossy().into_owned());
    }
    let data = crate::library::cover(path)?;
    let ext = if data.starts_with(b"\x89PNG") {
        "png"
    } else {
        "jpg"
    };
    let f = cover_cache(path, ext);
    let _ = std::fs::create_dir_all(f.parent()?);
    std::fs::write(&f, data).ok()?;
    Some(f.to_string_lossy().to_string())
}
