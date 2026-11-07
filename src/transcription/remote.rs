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
    /// Extra Deepgram query parameters, e.g. `{"numerals": "true"}`.
    pub extra_params: std::collections::BTreeMap<String, String>,
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
    if !config.enabled {
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
    let mut url = url::Url::parse(DEEPGRAM_URL).map_err(|e| e.to_string())?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("model", &config.model);
        if !config.language.is_empty() {
            query.append_pair("language", &config.language);
        }
        query.append_pair("smart_format", bool_str(config.smart_format));
        query.append_pair("punctuate", "true");
        if config.use_vocabulary {
            // Nova-3 keyterm prompting; Deepgram caps it around 100 terms.
            for term in crate::dictation::vocabulary::terms().iter().take(100) {
                if !term.trim().is_empty() {
                    query.append_pair("keyterm", term.trim());
                }
            }
        }
        for (key, value) in &config.extra_params {
            query.append_pair(key, value);
        }
    }
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

fn bool_str(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}
