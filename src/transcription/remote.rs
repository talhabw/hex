//! Fork patch (talhabw/hex): dictation through a remote speech-to-text API.
//!
//! When `~/.config/hex/remote-transcription.json` exists with `"enabled": true`,
//! Linux/Windows dictation skips live streaming and sends each finished clip
//! to the configured provider instead of the local model. Today the only
//! provider is Deepgram (`/v1/listen`, Nova-3 multilingual by default).
//!
//! ```json
//! {
//!   "enabled": true,
//!   "provider": "deepgram",
//!   "api_key": "dg_...",
//!   "model": "nova-3",
//!   "language": "multi",
//!   "smart_format": true,
//!   "use_vocabulary": true,
//!   "fallback_to_local": true,
//!   "timeout_seconds": 30
//! }
//! ```
//!
//! The file is read on every dictation, so edits apply without a restart.

use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

const FILE_NAME: &str = "remote-transcription.json";
const DEEPGRAM_URL: &str = "https://api.deepgram.com/v1/listen";
const DEEPGRAM_LIVE_URL: &str = "wss://api.deepgram.com/v1/listen";

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub(crate) struct RemoteConfig {
    pub enabled: bool,
    pub provider: String,
    pub api_key: String,
    pub model: String,
    pub language: String,
    pub smart_format: bool,
    pub use_vocabulary: bool,
    /// On a network or API failure, transcribe the clip with the local model.
    pub fallback_to_local: bool,
    pub timeout_seconds: u64,
    /// Extra Deepgram query parameters, e.g. `{"numerals": true}`; an array
    /// repeats the parameter (`{"language_hint": ["tr", "en"]}`).
    pub extra_params: std::collections::BTreeMap<String, serde_json::Value>,
    /// Most vocabulary terms sent as keyterms (Deepgram caps them at 500 tokens).
    pub max_keyterms: usize,
    /// Stream audio over Deepgram's live WebSocket while the shortcut is held,
    /// for live text and a faster finish. Off: send the whole clip on release.
    pub streaming: bool,
    /// How long a finished stream may take to deliver its last words before
    /// the whole clip is sent instead.
    pub stream_finish_ms: u64,
    /// Deepgram live `endpointing` (ms of silence that closes a segment).
    /// Deepgram's default is 10; 100 is its advice for code-switching, and
    /// longer segments give language detection more context.
    pub stream_endpointing_ms: u64,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "deepgram".into(),
            api_key: String::new(),
            model: "nova-3".into(),
            language: "multi".into(),
            smart_format: true,
            use_vocabulary: true,
            fallback_to_local: true,
            timeout_seconds: 30,
            extra_params: Default::default(),
            max_keyterms: 50,
            streaming: true,
            stream_finish_ms: 1_300,
            stream_endpointing_ms: 300,
        }
    }
}

fn config_path() -> Option<PathBuf> {
    Some(
        dirs::home_dir()?
            .join(".config")
            .join("hex")
            .join(FILE_NAME),
    )
}

/// The remote config when it is enabled and usable; `None` keeps Hex local.
pub(crate) fn active() -> Option<RemoteConfig> {
    let path = config_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    let mut config: RemoteConfig = match serde_json::from_str(&text) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "remote-transcription.json is invalid; using the local model");
            return None;
        }
    };
    // Labs › Deepgram transcription switches it; the file holds the key.
    if !config.enabled || !crate::app_settings::AppSettings::saved_labs().remote_transcription {
        return None;
    }
    if config.api_key.trim().is_empty() {
        config.api_key = std::env::var("DEEPGRAM_API_KEY").unwrap_or_default();
    }
    if config.api_key.trim().is_empty() {
        tracing::warn!("remote transcription has no api_key; using the local model");
        return None;
    }
    Some(config)
}

/// Transcribes a 16 kHz mono clip with the configured provider.
pub(crate) fn transcribe(config: &RemoteConfig, clip: &[f32]) -> Result<String, String> {
    match config.provider.as_str() {
        "deepgram" => deepgram(config, clip),
        other => Err(format!("unknown remote transcription provider {other:?}")),
    }
}

fn wav(clip: &[f32]) -> Result<Vec<u8>, String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut bytes = Cursor::new(Vec::with_capacity(clip.len() * 2 + 44));
    let mut writer = hound::WavWriter::new(&mut bytes, spec).map_err(|e| e.to_string())?;
    for sample in clip {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        writer.write_sample(value).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(bytes.into_inner())
}

fn deepgram(config: &RemoteConfig, clip: &[f32]) -> Result<String, String> {
    if clip.is_empty() {
        return Ok(String::new());
    }
    let body = wav(clip)?;
    let url = deepgram_url(DEEPGRAM_URL, config, &[])?;
    let agent = crate::net::agent(url.as_str(), |c| {
        c.timeout_global(Some(Duration::from_secs(config.timeout_seconds.max(5))))
            .http_status_as_error(false)
    });
    let started = std::time::Instant::now();
    let mut response = agent
        .post(url.as_str())
        .header("Authorization", &format!("Token {}", config.api_key.trim()))
        .header("Content-Type", "audio/wav")
        .send(&body[..])
        .map_err(|error| crate::net::unreachable(DEEPGRAM_URL, &error))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("could not read Deepgram's reply: {e}"))?;
    if !(200..300).contains(&status) {
        let detail: String = text.chars().take(300).collect();
        return Err(format!("Deepgram answered {status}: {detail}"));
    }
    let reply: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Deepgram reply is not JSON: {e}"))?;
    let transcript = reply
        .pointer("/results/channels/0/alternatives/0/transcript")
        .and_then(|value| value.as_str())
        .ok_or("Deepgram reply has no transcript")?
        .trim()
        .to_string();
    tracing::info!(
        audio_ms = clip.len() / 16,
        request_ms = started.elapsed().as_millis(),
        model = %config.model,
        "remote transcription finished"
    );
    Ok(transcript)
}

/// The Deepgram endpoint with this config's query, plus `extra` pairs.
fn deepgram_url(
    base: &str,
    config: &RemoteConfig,
    extra: &[(&str, &str)],
) -> Result<url::Url, String> {
    let mut url = url::Url::parse(base).map_err(|e| e.to_string())?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("model", &config.model);
        if !config.language.is_empty() {
            query.append_pair("language", &config.language);
        }
        query.append_pair("smart_format", bool_str(config.smart_format));
        query.append_pair("punctuate", "true");
        for (key, value) in extra {
            query.append_pair(key, value);
        }
        if config.use_vocabulary {
            // Nova-3 keyterm prompting: 500 tokens at most per request.
            for term in crate::dictation::vocabulary::terms()
                .iter()
                .take(config.max_keyterms)
            {
                if !term.trim().is_empty() {
                    query.append_pair("keyterm", term.trim());
                }
            }
        }
        for (key, value) in &config.extra_params {
            let values = match value {
                serde_json::Value::Array(items) => items.clone(),
                other => vec![other.clone()],
            };
            for value in values {
                match value {
                    serde_json::Value::Null => {}
                    serde_json::Value::String(text) => {
                        query.append_pair(key, &text);
                    }
                    other => {
                        query.append_pair(key, &other.to_string());
                    }
                }
            }
        }
    }
    Ok(url)
}

fn bool_str(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

pub(crate) use live::DeepgramStream;

mod live {
    //! Deepgram's live WebSocket as a dictation stream: audio goes out as it
    //! is captured, interim results become Hex's live text, and finishing
    //! closes the stream and waits briefly for its last words.

    use std::net::TcpStream;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use transcribe_cpp::CancelToken;
    use tungstenite::client::IntoClientRequest;
    use tungstenite::stream::MaybeTlsStream;
    use tungstenite::{Message, WebSocket};

    use super::{DEEPGRAM_LIVE_URL, RemoteConfig, deepgram_url};
    use crate::transcription::engine::Partial;

    const READ_POLL: Duration = Duration::from_millis(15);

    enum Out {
        Audio(Vec<u8>),
        Close,
    }

    #[derive(Default)]
    struct Text {
        /// Final segments, joined.
        committed: String,
        /// The latest interim hypothesis after `committed`.
        tentative: String,
        /// The socket closed after CloseStream (or failed).
        done: bool,
        error: Option<String>,
    }

    pub(crate) struct DeepgramStream {
        out: Sender<Out>,
        text: Arc<(Mutex<Text>, Condvar)>,
        cancel: CancelToken,
        finish_wait: Duration,
        started: Instant,
        audio_samples: usize,
    }

    impl DeepgramStream {
        /// Starts connecting at once; audio fed before the socket opens waits
        /// in the queue.
        pub(crate) fn start(config: &RemoteConfig, cancel: CancelToken) -> Result<Self, String> {
            let endpointing = config.stream_endpointing_ms.to_string();
            let url = deepgram_url(
                DEEPGRAM_LIVE_URL,
                config,
                &[
                    ("encoding", "linear16"),
                    ("sample_rate", "16000"),
                    ("channels", "1"),
                    ("interim_results", "true"),
                    ("endpointing", &endpointing),
                ],
            )?;
            let mut request = url
                .as_str()
                .into_client_request()
                .map_err(|e| e.to_string())?;
            request.headers_mut().insert(
                "Authorization",
                format!("Token {}", config.api_key.trim())
                    .parse()
                    .map_err(|_| "invalid api_key".to_string())?,
            );
            let (out, outgoing) = mpsc::channel();
            let text: Arc<(Mutex<Text>, Condvar)> = Arc::default();
            let shared = text.clone();
            let thread_cancel = cancel.clone();
            thread::Builder::new()
                .name("hex-deepgram-live".into())
                .spawn(move || {
                    let result = run(request, outgoing, &shared, &thread_cancel);
                    let (lock, ready) = &*shared;
                    let mut text = lock.lock().unwrap_or_else(|e| e.into_inner());
                    if let Err(error) = result {
                        tracing::warn!(%error, "Deepgram live stream failed");
                        text.error = Some(error);
                    }
                    text.done = true;
                    ready.notify_all();
                })
                .map_err(|e| e.to_string())?;
            Ok(Self {
                out,
                text,
                cancel,
                finish_wait: Duration::from_millis(config.stream_finish_ms.max(200)),
                started: Instant::now(),
                audio_samples: 0,
            })
        }

        fn failed(&self) -> Option<String> {
            let text = self.text.0.lock().unwrap_or_else(|e| e.into_inner());
            text.error.clone()
        }
    }

    impl Drop for DeepgramStream {
        fn drop(&mut self) {
            let _ = self.out.send(Out::Close);
        }
    }

    impl crate::transcription::engine::StreamFeed for DeepgramStream {
        fn feed(&mut self, samples: &[f32]) -> Result<(), String> {
            if let Some(error) = self.failed() {
                return Err(error);
            }
            self.audio_samples += samples.len();
            let mut bytes = Vec::with_capacity(samples.len() * 2);
            for sample in samples {
                let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            self.out
                .send(Out::Audio(bytes))
                .map_err(|_| "the Deepgram stream closed".to_string())
        }

        fn finalize(&mut self) -> Result<String, String> {
            let closing = Instant::now();
            let _ = self.out.send(Out::Close);
            let (lock, ready) = &*self.text;
            let mut text = lock.lock().unwrap_or_else(|e| e.into_inner());
            while !text.done {
                if self.cancel.is_cancelled() {
                    return Err("the Deepgram stream was cancelled".into());
                }
                let left = self.finish_wait.saturating_sub(closing.elapsed());
                if left.is_zero() {
                    return Err("the Deepgram stream missed its finish deadline".into());
                }
                text = ready
                    .wait_timeout(text, left.min(Duration::from_millis(50)))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            if let Some(error) = &text.error {
                return Err(error.clone());
            }
            let full = join(&text.committed, &text.tentative);
            tracing::info!(
                audio_ms = self.audio_samples / 16,
                stream_ms = self.started.elapsed().as_millis(),
                finish_ms = closing.elapsed().as_millis(),
                "Deepgram live stream finished"
            );
            Ok(full)
        }

        fn partial(&self) -> Partial {
            let text = self.text.0.lock().unwrap_or_else(|e| e.into_inner());
            Partial {
                stable: text.committed.clone(),
                tentative: if text.committed.is_empty() || text.tentative.is_empty() {
                    text.tentative.clone()
                } else {
                    format!(" {}", text.tentative)
                },
            }
        }
    }

    fn join(a: &str, b: &str) -> String {
        match (a.is_empty(), b.is_empty()) {
            (true, _) => b.to_string(),
            (_, true) => a.to_string(),
            _ => format!("{a} {b}"),
        }
    }

    fn run(
        request: tungstenite::handshake::client::Request,
        outgoing: Receiver<Out>,
        shared: &Arc<(Mutex<Text>, Condvar)>,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let (mut socket, _) = tungstenite::connect(request).map_err(describe)?;
        set_read_timeout(&socket, Some(READ_POLL));
        let mut closing = false;
        let mut close_sent_at: Option<Instant> = None;
        loop {
            if cancel.is_cancelled() && !closing {
                return Err("cancelled".into());
            }
            // Send everything queued.
            loop {
                match outgoing.try_recv() {
                    Ok(Out::Audio(bytes)) if !closing => {
                        socket
                            .send(Message::Binary(bytes.into()))
                            .map_err(describe)?;
                    }
                    Ok(Out::Audio(_)) => {}
                    Ok(Out::Close) | Err(mpsc::TryRecvError::Disconnected) if !closing => {
                        closing = true;
                        close_sent_at = Some(Instant::now());
                        socket
                            .send(Message::Text(r#"{"type":"CloseStream"}"#.into()))
                            .map_err(describe)?;
                        break;
                    }
                    Ok(Out::Close) => {}
                    Err(mpsc::TryRecvError::Disconnected) => break,
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            if close_sent_at.is_some_and(|at| at.elapsed() > Duration::from_secs(10)) {
                return Ok(());
            }
            match socket.read() {
                Ok(Message::Text(body)) => handle(&body, shared),
                Ok(Message::Close(_)) => return Ok(()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if !closing {
                        // Idle: wait briefly for audio instead of spinning.
                        match outgoing.recv_timeout(Duration::from_millis(1)) {
                            Ok(Out::Audio(bytes)) => socket
                                .send(Message::Binary(bytes.into()))
                                .map_err(describe)?,
                            Ok(Out::Close) => {
                                closing = true;
                                close_sent_at = Some(Instant::now());
                                socket
                                    .send(Message::Text(r#"{"type":"CloseStream"}"#.into()))
                                    .map_err(describe)?;
                            }
                            Err(RecvTimeoutError::Timeout) => {}
                            Err(RecvTimeoutError::Disconnected) => {}
                        }
                    }
                }
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    return Ok(());
                }
                Err(error) => return Err(describe(error)),
            }
        }
    }

    fn handle(body: &str, shared: &Arc<(Mutex<Text>, Condvar)>) {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(body) else {
            return;
        };
        if message.get("type").and_then(|t| t.as_str()) != Some("Results") {
            return;
        }
        let transcript = message
            .pointer("/channel/alternatives/0/transcript")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim();
        let is_final = message
            .get("is_final")
            .and_then(|f| f.as_bool())
            .unwrap_or(false);
        let (lock, ready) = &**shared;
        let mut text = lock.lock().unwrap_or_else(|e| e.into_inner());
        if is_final {
            if !transcript.is_empty() {
                text.committed = join(&text.committed, transcript);
            }
            text.tentative.clear();
        } else {
            text.tentative = transcript.to_string();
        }
        ready.notify_all();
    }

    fn set_read_timeout(socket: &WebSocket<MaybeTlsStream<TcpStream>>, timeout: Option<Duration>) {
        let tcp = match socket.get_ref() {
            MaybeTlsStream::Plain(tcp) => tcp,
            MaybeTlsStream::NativeTls(tls) => tls.get_ref(),
            _ => return,
        };
        let _ = tcp.set_read_timeout(timeout);
    }

    fn describe(error: tungstenite::Error) -> String {
        match error {
            tungstenite::Error::Http(response) => {
                let body = response
                    .body()
                    .as_ref()
                    .map(|b| {
                        String::from_utf8_lossy(b)
                            .chars()
                            .take(300)
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                format!(
                    "Deepgram refused the stream ({}): {body}",
                    response.status()
                )
            }
            other => format!("Deepgram stream: {other}"),
        }
    }
}
