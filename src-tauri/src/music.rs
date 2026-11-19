//! Desktop audio lives outside WebViews. No device is opened until playback;
//! stopping releases it, and idle workers block on channels instead of polling.
use serde::Serialize;
use tauri::{AppHandle, Runtime};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    title: &'static str,
    artist: &'static str,
    file: &'static str,
}

const TRACKS: [Track; 5] = [
    Track { title: "Midnight Club", artist: "Alex Morgan", file: "alex-morgan-midnight-club-568164.mp3" },
    Track { title: "Lofi Polaroids", artist: "FASSounds", file: "fassounds-lofi-polaroids-632981.mp3" },
    Track { title: "Tokyo City Rain", artist: "LofiDreams", file: "lofidreams-tokyo-city-rain-332743.mp3" },
    Track { title: "Daily Routine Work", artist: "LoFiHour", file: "lofihour-daily-routine-work-116972.mp3" },
    Track { title: "Ghost Town", artist: "Purrple Cat", file: "purrplecat-ghost-town-518603.mp3" },
];

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicStatus {
    tracks: &'static [Track],
    track: usize,
    playing: bool,
    volume: f32,
    error: Option<String>,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop {
    use super::*;
    use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink};
    use std::{fs::File, path::PathBuf, sync::{mpsc, Arc}};
    use tauri::{Emitter, Manager};

    type Reply = mpsc::Sender<Result<MusicStatus, String>>;
    pub enum Message {
        Control(String, Option<f32>, Option<Reply>),
        Finished(u64),
    }
    pub struct MusicRuntime(pub mpsc::Sender<Message>);

    struct Playback {
        sink: Arc<Sink>,
        // The output stream must outlive its sink.
        _stream: OutputStream,
    }

    struct Player {
        status: MusicStatus,
        playback: Option<Playback>,
        generation: u64,
        resources: PathBuf,
        preferences: PathBuf,
        tx: mpsc::Sender<Message>,
    }

    impl Player {
        fn stop(&mut self) {
            self.generation = self.generation.wrapping_add(1);
            if let Some(playback) = self.playback.take() {
                playback.sink.stop();
                drop(playback);
            }
            self.status.playing = false;
        }

        fn play(&mut self) -> Result<(), String> {
            let file = File::open(self.resources.join(TRACKS[self.status.track].file))
                .map_err(|error| format!("Unable to open music: {error}"))?;
            let source = Decoder::try_from(file).map_err(|error| error.to_string())?;
            let mut stream = OutputStreamBuilder::open_default_stream().map_err(|error| error.to_string())?;
            stream.log_on_drop(false);
            let sink = Arc::new(Sink::connect_new(stream.mixer()));
            sink.pause();
            sink.set_volume(self.status.volume);
            sink.append(source);
            let generation = self.generation;
            let tx = self.tx.clone();
            let waiter = sink.clone();
            // Blocks until the actual output queue ends (or is stopped).
            // No timers or track-completion polling in the background.
            std::thread::Builder::new().name("music-completion".into()).spawn(move || {
                waiter.sleep_until_end();
                let _ = tx.send(Message::Finished(generation));
            }).map_err(|error| error.to_string())?;
            sink.play();
            self.playback = Some(Playback { sink, _stream: stream });
            self.status.playing = true;
            Ok(())
        }

        fn control(&mut self, action: &str, value: Option<f32>) -> Result<(), String> {
            match action {
                "status" => return Ok(()),
                "toggle" => {
                    if self.status.playing { self.stop(); } else { self.play()?; }
                }
                "stop" => {
                    self.stop();
                }
                "volume" => {
                    let volume = value.filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
                        .ok_or("Volume must be between 0 and 1")?;
                    if let Some(parent) = self.preferences.parent() {
                        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                    }
                    std::fs::write(&self.preferences, volume.to_string()).map_err(|error| error.to_string())?;
                    self.status.volume = volume;
                    if let Some(playback) = &self.playback { playback.sink.set_volume(volume); }
                }
                _ => return Err("Unsupported music action".into()),
            }
            Ok(())
        }

        fn advance(&mut self) -> Result<(), String> {
            self.stop();
            self.status.track = (self.status.track + 1) % TRACKS.len();
            self.play()
        }
    }

    pub fn start<R: Runtime>(app: &AppHandle<R>) -> Result<(), Box<dyn std::error::Error>> {
        let resources = app.path().resource_dir()?.join("resources/music");
        let preferences = app.path().app_data_dir()?.join("music-volume");
        let volume = std::fs::read_to_string(&preferences).ok()
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value)).unwrap_or(0.4);
        let (tx, rx) = mpsc::channel();
        app.manage(MusicRuntime(tx.clone()));
        let app = app.clone();
        std::thread::Builder::new().name("music-player".into()).spawn(move || {
            let mut player = Player {
                status: MusicStatus { tracks: &TRACKS, track: 0, playing: false, volume, error: None },
                playback: None, generation: 0, resources, preferences, tx,
            };
            while let Ok(message) = rx.recv() {
                let (result, reply, changed) = match message {
                    Message::Control(action, value, reply) => {
                        let result = player.control(&action, value);
                        (result, reply, action != "status")
                    }
                    Message::Finished(generation) => {
                        if generation != player.generation || !player.status.playing { continue; }
                        (player.advance(), None, true)
                    }
                };
                if changed {
                    if result.is_err() { player.stop(); }
                    player.status.error = result.as_ref().err().cloned();
                    let _ = app.emit("music-changed", &player.status);
                    if let Some(tray) = app.tray_by_id("main-tray") {
                        let tooltip = if player.status.playing {
                            format!("OneByOne · {} — {}", TRACKS[player.status.track].title, TRACKS[player.status.track].artist)
                        } else { "OneByOne".into() };
                        let _ = tray.set_tooltip(Some(tooltip));
                    }
                }
                if let Some(reply) = reply { let _ = reply.send(result.map(|_| player.status.clone())); }
            }
            player.stop();
        })?;
        Ok(())
    }

    pub fn tray_control<R: Runtime>(app: &AppHandle<R>, action: &str) {
        let _ = app.state::<MusicRuntime>().0.send(Message::Control(action.into(), None, None));
    }

    pub async fn request<R: Runtime>(app: AppHandle<R>, action: String, value: Option<f32>) -> Result<MusicStatus, String> {
        let tx = app.state::<MusicRuntime>().0.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let (reply, rx) = mpsc::channel();
            tx.send(Message::Control(action, value, Some(reply))).map_err(|error| error.to_string())?;
            rx.recv().map_err(|error| error.to_string())?
        }).await.map_err(|error| error.to_string())?
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use rodio::Source;
        use std::time::Duration;

        #[test]
        fn bundled_tracks_decode_and_support_resume_seeking() {
            let resources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/music");
            for track in &TRACKS {
                let mut source = Decoder::try_from(File::open(resources.join(track.file)).unwrap()).unwrap();
                assert!(source.channels() > 0, "{}", track.file);
                assert!(source.sample_rate() > 0, "{}", track.file);
                assert!(source.total_duration().unwrap() > Duration::from_secs(10), "{}", track.file);
                assert!(source.by_ref().take(8192).all(f32::is_finite), "{}", track.file);
                source.try_seek(Duration::from_secs(5)).unwrap();
                assert!(source.next().unwrap().is_finite(), "{}", track.file);
                source.try_seek(Duration::ZERO).unwrap();
                assert!(source.next().unwrap().is_finite(), "{}", track.file);
            }
        }

        #[test]
        fn idle_controls_stop_and_persist_volume_without_opening_audio() {
            let dir = tempfile::tempdir().unwrap();
            let (tx, _rx) = mpsc::channel();
            let mut player = Player {
                status: MusicStatus { tracks: &TRACKS, track: 0, playing: false, volume: 0.4, error: None },
                playback: None, generation: 0,
                resources: dir.path().into(), preferences: dir.path().join("music-volume"), tx,
            };
            assert!(player.control("previous", None).is_err());
            assert!(player.control("next", None).is_err());
            assert_eq!(player.status.track, 0);
            for value in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
                assert!(player.control("volume", Some(value)).is_err());
            }
            player.control("volume", Some(0.25)).unwrap();
            assert_eq!(std::fs::read_to_string(&player.preferences).unwrap(), "0.25");
            assert_eq!(player.status.volume, 0.25);
            player.control("stop", None).unwrap();
            assert!(!player.status.playing);
            assert!(player.playback.is_none());
            player.status.playing = true;
            player.control("toggle", None).unwrap();
            assert!(!player.status.playing);
            assert!(player.playback.is_none());
            assert!(player.control("toggle", None).is_err()); // Missing file, no device opened.
            assert!(!player.status.playing);
            assert!(player.playback.is_none());
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use desktop::{start, tray_control};

#[tauri::command]
pub async fn music_control<R: Runtime>(app: AppHandle<R>, action: String, value: Option<f32>) -> Result<MusicStatus, String> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    { desktop::request(app, action, value).await }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    { let _ = (app, action, value); Err("Music is only available on desktop".into()) }
}
