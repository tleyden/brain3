use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use brain3_core::ports::native_mcp_tool::{NativeMcpTool, NativeMcpToolError, NativeMcpToolOutput};
use serde::Deserialize;
use serde_json::{json, Value};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::FormatOptions;
use symphonia::core::formats::TrackType;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const TARGET_SAMPLE_RATE: u32 = 16_000;
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhisperTranscribeConfig {
    pub model_path: PathBuf,
    pub max_audio_bytes: u64,
}

pub struct WhisperTranscribeTool {
    config: WhisperTranscribeConfig,
    http_client: reqwest::Client,
    transcriber: Arc<dyn AudioTranscriber>,
}

impl WhisperTranscribeTool {
    pub fn new(config: WhisperTranscribeConfig) -> Self {
        let transcriber = Arc::new(CachedWhisperTranscriber::new(config.model_path.clone()));
        Self::with_transcriber(config, transcriber)
    }

    fn with_transcriber(
        config: WhisperTranscribeConfig,
        transcriber: Arc<dyn AudioTranscriber>,
    ) -> Self {
        Self::with_transcriber_and_timeouts(
            config,
            transcriber,
            HTTP_CONNECT_TIMEOUT,
            HTTP_TOTAL_TIMEOUT,
        )
    }

    fn with_transcriber_and_timeouts(
        config: WhisperTranscribeConfig,
        transcriber: Arc<dyn AudioTranscriber>,
        connect_timeout: Duration,
        total_timeout: Duration,
    ) -> Self {
        let http_client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(connect_timeout)
            .timeout(total_timeout)
            .build()
            .expect("failed to build reqwest client for native transcription tool");
        Self {
            config,
            http_client,
            transcriber,
        }
    }

    async fn execute(
        &self,
        arguments: Value,
    ) -> Result<NativeMcpToolOutput, WhisperTranscribeError> {
        let args: WhisperToolArguments = serde_json::from_value(arguments)
            .map_err(|error| WhisperTranscribeError::InvalidArguments(error.to_string()))?;
        let download_url = reqwest::Url::parse(&args.audio_file.download_url)
            .map_err(|error| WhisperTranscribeError::InvalidDownloadUrl(error.to_string()))?;
        if !matches!(download_url.scheme(), "http" | "https") {
            return Err(WhisperTranscribeError::InvalidDownloadUrl(
                "download_url must use http or https".into(),
            ));
        }

        let host = download_url.host_str().unwrap_or("unknown").to_string();
        tracing::info!(
            file_id = %args.audio_file.file_id,
            file_name = ?args.audio_file.file_name,
            mime_type = ?args.audio_file.mime_type,
            host = %host,
            max_audio_bytes = self.config.max_audio_bytes,
            "native whisper transcription: downloading audio"
        );

        let start = Instant::now();
        let (_temp_dir, audio_path, bytes_written) = self
            .download_to_temp_file(download_url, &args.audio_file)
            .await?;
        tracing::info!(
            file_id = %args.audio_file.file_id,
            bytes_written,
            elapsed_ms = start.elapsed().as_millis() as u64,
            "native whisper transcription: audio download complete"
        );

        let decode_start = Instant::now();
        let samples = decode_audio_file_to_whisper_pcm(&audio_path)?;
        tracing::info!(
            file_id = %args.audio_file.file_id,
            sample_count = samples.len(),
            duration_ms = samples.len() as u64 * 1000 / TARGET_SAMPLE_RATE as u64,
            elapsed_ms = decode_start.elapsed().as_millis() as u64,
            "native whisper transcription: audio decoded and resampled"
        );

        let transcriber = Arc::clone(&self.transcriber);
        let inference_start = Instant::now();
        let transcript = tokio::task::spawn_blocking(move || transcriber.transcribe(samples))
            .await
            .map_err(|error| WhisperTranscribeError::Inference(error.to_string()))??;
        tracing::info!(
            file_id = %args.audio_file.file_id,
            transcript_chars = transcript.chars().count(),
            elapsed_ms = inference_start.elapsed().as_millis() as u64,
            "native whisper transcription: inference complete"
        );

        Ok(NativeMcpToolOutput::text(transcript))
    }

    async fn download_to_temp_file(
        &self,
        download_url: reqwest::Url,
        audio_file: &OpenAIFileReferenceInput,
    ) -> Result<(TempDir, PathBuf, u64), WhisperTranscribeError> {
        let temp_dir = tempfile::Builder::new()
            .prefix("brain3_audio_")
            .tempdir()
            .map_err(|error| WhisperTranscribeError::TempFile(error.to_string()))?;
        let audio_path = temp_dir.path().join(destination_filename(audio_file));
        let mut output = tokio::fs::File::create(&audio_path)
            .await
            .map_err(|error| WhisperTranscribeError::TempFile(error.to_string()))?;

        let mut response = self
            .http_client
            .get(download_url)
            .send()
            .await
            .map_err(|error| WhisperTranscribeError::Download(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            return Err(WhisperTranscribeError::Download(format!(
                "download_url returned HTTP {status}"
            )));
        }

        let mut bytes_written: u64 = 0;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| WhisperTranscribeError::Download(error.to_string()))?
        {
            bytes_written = bytes_written
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| WhisperTranscribeError::SizeLimitExceeded {
                    max_audio_bytes: self.config.max_audio_bytes,
                })?;
            if bytes_written > self.config.max_audio_bytes {
                return Err(WhisperTranscribeError::SizeLimitExceeded {
                    max_audio_bytes: self.config.max_audio_bytes,
                });
            }
            output
                .write_all(&chunk)
                .await
                .map_err(|error| WhisperTranscribeError::TempFile(error.to_string()))?;
        }

        output
            .flush()
            .await
            .map_err(|error| WhisperTranscribeError::TempFile(error.to_string()))?;

        Ok((temp_dir, audio_path, bytes_written))
    }
}

#[async_trait::async_trait]
impl NativeMcpTool for WhisperTranscribeTool {
    fn name(&self) -> &str {
        "transcribe_audio_file"
    }

    fn description(&self) -> &str {
        "Transcribe an uploaded audio file using native Whisper inference."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "audio_file": {
                    "type": "object",
                    "additionalProperties": false,
                    "description": "OpenAI file reference for the uploaded audio file",
                    "properties": {
                        "download_url": {
                            "type": "string",
                            "format": "uri",
                            "description": "Temporary authorized download URL for the uploaded file"
                        },
                        "file_id": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": 200,
                            "description": "OpenAI file identifier"
                        },
                        "mime_type": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": 200,
                            "description": "Optional MIME type for the uploaded file"
                        },
                        "file_name": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": 500,
                            "description": "Optional original filename for the uploaded file"
                        }
                    },
                    "required": ["download_url", "file_id"]
                }
            },
            "required": ["audio_file"]
        })
    }

    fn meta(&self) -> Option<Value> {
        Some(json!({ "openai/fileParams": ["audio_file"] }))
    }

    async fn call(&self, arguments: Value) -> Result<NativeMcpToolOutput, NativeMcpToolError> {
        self.execute(arguments).await.map_err(Into::into)
    }
}

trait AudioTranscriber: Send + Sync {
    fn transcribe(&self, samples: Vec<f32>) -> Result<String, WhisperTranscribeError>;
}

struct CachedWhisperTranscriber {
    model_path: PathBuf,
    context: Mutex<Option<WhisperContext>>,
}

impl CachedWhisperTranscriber {
    fn new(model_path: PathBuf) -> Self {
        Self {
            model_path,
            context: Mutex::new(None),
        }
    }
}

impl AudioTranscriber for CachedWhisperTranscriber {
    fn transcribe(&self, samples: Vec<f32>) -> Result<String, WhisperTranscribeError> {
        let mut context_guard = self.context.lock().map_err(|_| {
            WhisperTranscribeError::Inference("whisper context lock poisoned".into())
        })?;

        if context_guard.is_none() {
            tracing::info!(
                model_path = %self.model_path.display(),
                backend = whisper_backend_label(),
                "native whisper transcription: loading model"
            );
            let context = WhisperContext::new_with_params(
                &self.model_path,
                WhisperContextParameters::default(),
            )
            .map_err(|error| WhisperTranscribeError::ModelLoad(error.to_string()))?;
            *context_guard = Some(context);
        }

        let context = context_guard
            .as_ref()
            .expect("context should be initialized above");
        let mut state = context
            .create_state()
            .map_err(|error| WhisperTranscribeError::Inference(error.to_string()))?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 0 });
        params.set_language(Some("en"));
        params.set_translate(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state
            .full(params, &samples)
            .map_err(|error| WhisperTranscribeError::Inference(error.to_string()))?;

        let mut transcript = String::new();
        for segment in state.as_iter() {
            transcript.push_str(
                segment
                    .to_str_lossy()
                    .map_err(|error| WhisperTranscribeError::Inference(error.to_string()))?
                    .as_ref(),
            );
        }

        Ok(transcript.trim().to_string())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WhisperToolArguments {
    audio_file: OpenAIFileReferenceInput,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenAIFileReferenceInput {
    download_url: String,
    file_id: String,
    mime_type: Option<String>,
    file_name: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum WhisperTranscribeError {
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("invalid download_url: {0}")]
    InvalidDownloadUrl(String),
    #[error("audio download failed: {0}")]
    Download(String),
    #[error("audio download exceeds configured limit of {max_audio_bytes} bytes")]
    SizeLimitExceeded { max_audio_bytes: u64 },
    #[error("temporary audio file error: {0}")]
    TempFile(String),
    #[error("audio decode failed: {0}")]
    Decode(String),
    #[error("whisper model load failed: {0}")]
    ModelLoad(String),
    #[error("whisper inference failed: {0}")]
    Inference(String),
}

impl From<WhisperTranscribeError> for NativeMcpToolError {
    fn from(error: WhisperTranscribeError) -> Self {
        match error {
            WhisperTranscribeError::InvalidArguments(message)
            | WhisperTranscribeError::InvalidDownloadUrl(message) => {
                NativeMcpToolError::InvalidArguments(message)
            }
            other => NativeMcpToolError::ExecutionFailed(other.to_string()),
        }
    }
}

fn decode_audio_file_to_whisper_pcm(path: &Path) -> Result<Vec<f32>, WhisperTranscribeError> {
    let file =
        File::open(path).map_err(|error| WhisperTranscribeError::Decode(error.to_string()))?;
    let media_source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            media_source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|error| WhisperTranscribeError::Decode(error.to_string()))?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| WhisperTranscribeError::Decode("no audio track found".into()))?;
    let track_id = track.id;
    let audio_params = track
        .codec_params
        .as_ref()
        .ok_or_else(|| WhisperTranscribeError::Decode("audio codec parameters missing".into()))?
        .audio()
        .ok_or_else(|| WhisperTranscribeError::Decode("audio codec parameters missing".into()))?;
    let sample_rate = audio_params
        .sample_rate
        .ok_or_else(|| WhisperTranscribeError::Decode("audio sample rate missing".into()))?;
    let channel_count = audio_params
        .channels
        .as_ref()
        .map(|channels| channels.count())
        .ok_or_else(|| WhisperTranscribeError::Decode("audio channel layout missing".into()))?;
    if channel_count == 0 {
        return Err(WhisperTranscribeError::Decode(
            "audio channel layout has no channels".into(),
        ));
    }

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(audio_params, &AudioDecoderOptions::default())
        .map_err(|error| WhisperTranscribeError::Decode(error.to_string()))?;

    let mut mono_samples = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(error) => return Err(WhisperTranscribeError::Decode(error.to_string())),
        };

        if packet.track_id != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(error)) => {
                tracing::debug!(
                    error = error,
                    "native whisper transcription: skipped undecodable packet"
                );
                continue;
            }
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(error) => return Err(WhisperTranscribeError::Decode(error.to_string())),
        };

        let channels = decoded.spec().channels().count();
        if channels == 0 {
            continue;
        }
        let mut interleaved = vec![0.0f32; decoded.samples_interleaved()];
        decoded.copy_to_slice_interleaved(&mut interleaved);
        mono_samples.extend(downmix_interleaved_to_mono(&interleaved, channels));
    }

    if mono_samples.is_empty() {
        return Err(WhisperTranscribeError::Decode(
            "audio file did not decode to any samples".into(),
        ));
    }

    Ok(resample_linear(
        &mono_samples,
        sample_rate,
        TARGET_SAMPLE_RATE,
    ))
}

fn downmix_interleaved_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

fn resample_linear(samples: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if source_rate == target_rate || samples.is_empty() {
        return samples.to_vec();
    }

    let output_len = ((samples.len() as u64 * target_rate as u64) / source_rate as u64) as usize;
    if output_len == 0 {
        return Vec::new();
    }

    let ratio = source_rate as f64 / target_rate as f64;
    let mut output = Vec::with_capacity(output_len);
    for output_index in 0..output_len {
        let source_position = output_index as f64 * ratio;
        let lower = source_position.floor() as usize;
        let upper = (lower + 1).min(samples.len() - 1);
        let fraction = (source_position - lower as f64) as f32;
        output.push(samples[lower] * (1.0 - fraction) + samples[upper] * fraction);
    }

    output
}

fn destination_filename(audio_file: &OpenAIFileReferenceInput) -> String {
    let mut name = audio_file
        .file_name
        .as_deref()
        .and_then(|name| Path::new(name).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("audio")
        .trim()
        .to_string();
    if name.is_empty() {
        name = "audio".into();
    }
    name
}

fn whisper_backend_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "metal"
    }

    #[cfg(not(target_os = "macos"))]
    {
        "cpu"
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{body::Body, http::StatusCode, response::Response, routing::get, Router};
    use brain3_core::application::native_mcp_tool_registry::NativeMcpToolRegistry;
    use brain3_core::ports::native_mcp_tool::NativeMcpTool;
    use serde_json::json;
    use tokio::net::TcpListener;

    use super::*;

    #[derive(Default)]
    struct FakeTranscriber {
        calls: Mutex<Vec<Vec<f32>>>,
    }

    impl AudioTranscriber for FakeTranscriber {
        fn transcribe(&self, samples: Vec<f32>) -> Result<String, WhisperTranscribeError> {
            self.calls
                .lock()
                .expect("fake transcriber lock should succeed")
                .push(samples);
            Ok("hello from native whisper".into())
        }
    }

    #[tokio::test]
    async fn call_downloads_decodes_resamples_and_returns_transcript() {
        let wav = pcm_wav(8_000, 1, 800);
        let download_url = serve_once(wav).await;
        let fake = Arc::new(FakeTranscriber::default());
        let tool = WhisperTranscribeTool::with_transcriber(
            WhisperTranscribeConfig {
                model_path: "/tmp/model.bin".into(),
                max_audio_bytes: 1_000_000,
            },
            fake.clone(),
        );

        let output = tool
            .call(json!({
                "audio_file": {
                    "download_url": download_url,
                    "file_id": "file_123",
                    "mime_type": "audio/wav",
                    "file_name": "sample.wav"
                }
            }))
            .await
            .expect("tool call should succeed");

        assert!(!output.is_error);
        assert_eq!(output.content[0].text, "hello from native whisper");
        let calls = fake
            .calls
            .lock()
            .expect("fake transcriber lock should succeed");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].len(), 1_600);
    }

    #[tokio::test]
    async fn call_rejects_downloads_larger_than_configured_cap() {
        let download_url = serve_once(vec![0u8; 128]).await;
        let fake = Arc::new(FakeTranscriber::default());
        let tool = WhisperTranscribeTool::with_transcriber(
            WhisperTranscribeConfig {
                model_path: "/tmp/model.bin".into(),
                max_audio_bytes: 64,
            },
            fake.clone(),
        );

        let error = tool
            .call(json!({
                "audio_file": {
                    "download_url": download_url,
                    "file_id": "file_oversized"
                }
            }))
            .await
            .expect_err("oversized download should fail");

        assert!(error.to_string().contains("exceeds configured limit"));
        assert!(fake
            .calls
            .lock()
            .expect("fake transcriber lock should succeed")
            .is_empty());
    }

    #[tokio::test]
    async fn call_times_out_stalled_downloads() {
        let download_url = serve_stalled_response().await;
        let fake = Arc::new(FakeTranscriber::default());
        let tool = WhisperTranscribeTool::with_transcriber_and_timeouts(
            WhisperTranscribeConfig {
                model_path: "/tmp/model.bin".into(),
                max_audio_bytes: 1_000_000,
            },
            fake.clone(),
            Duration::from_millis(50),
            Duration::from_millis(200),
        );

        let start = Instant::now();
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            tool.call(json!({
                "audio_file": {
                    "download_url": download_url,
                    "file_id": "file_stalled"
                }
            })),
        )
        .await
        .expect("tool call should finish within the test deadline")
        .expect_err("stalled download should fail");

        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(error.to_string().contains("audio download failed"));
        assert!(fake
            .calls
            .lock()
            .expect("fake transcriber lock should succeed")
            .is_empty());
    }

    #[test]
    fn input_schema_declares_openai_audio_file_reference() {
        let fake = Arc::new(FakeTranscriber::default());
        let tool = WhisperTranscribeTool::with_transcriber(
            WhisperTranscribeConfig {
                model_path: "/tmp/model.bin".into(),
                max_audio_bytes: 64,
            },
            fake,
        );

        assert_eq!(tool.name(), "transcribe_audio_file");
        let schema = tool.input_schema();

        assert_eq!(schema["required"], json!(["audio_file"]));
        assert_eq!(
            schema["properties"]["audio_file"]["required"],
            json!(["download_url", "file_id"])
        );
    }

    #[test]
    fn registry_schema_advertises_openai_file_metadata() {
        let fake = Arc::new(FakeTranscriber::default());
        let tool: Arc<dyn NativeMcpTool> = Arc::new(WhisperTranscribeTool::with_transcriber(
            WhisperTranscribeConfig {
                model_path: "/tmp/model.bin".into(),
                max_audio_bytes: 64,
            },
            fake,
        ));
        let registry = NativeMcpToolRegistry::new(vec![tool]);

        let schemas = registry.list_schemas();

        assert_eq!(schemas.len(), 1);
        assert_eq!(
            schemas[0]["_meta"],
            json!({
                "openai/fileParams": ["audio_file"]
            })
        );
    }

    async fn serve_once(body: Vec<u8>) -> String {
        let app = Router::new().route(
            "/audio",
            get(move || async move {
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "audio/wav")
                    .body(Body::from(body))
                    .expect("response should build")
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let addr = listener
            .local_addr()
            .expect("local addr should be available");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should serve");
        });
        format!("http://{addr}/audio")
    }

    async fn serve_stalled_response() -> String {
        let app = Router::new().route(
            "/audio",
            get(|| async move {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "audio/wav")
                    .body(Body::empty())
                    .expect("response should build")
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let addr = listener
            .local_addr()
            .expect("local addr should be available");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should serve");
        });
        format!("http://{addr}/audio")
    }

    fn pcm_wav(sample_rate: u32, channels: u16, frames: usize) -> Vec<u8> {
        let bytes_per_sample = 2u16;
        let data_len = frames * channels as usize * bytes_per_sample as usize;
        let mut wav = Vec::with_capacity(44 + data_len);

        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(
            &(sample_rate * channels as u32 * bytes_per_sample as u32).to_le_bytes(),
        );
        wav.extend_from_slice(&(channels * bytes_per_sample).to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data_len as u32).to_le_bytes());

        for frame in 0..frames {
            let sample = (((frame as f32 / sample_rate as f32) * 440.0 * std::f32::consts::TAU)
                .sin()
                * i16::MAX as f32
                * 0.25) as i16;
            for _ in 0..channels {
                wav.extend_from_slice(&sample.to_le_bytes());
            }
        }

        wav
    }
}
