//! The machine-learning models WaveCraft can use, pinned by revision, size and sha256, and the
//! `Models` trait the engine calls them through (implemented by the `wave-ml` crate, which links the
//! native runtimes; the engine itself never does).

use crate::lyrics::Word;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Separation { stems: u8 },
    Transcription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: &'static str,
    /// File name in the models folder.
    pub file: &'static str,
    /// Download URL, pinned to a repository revision.
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
    pub licence: &'static str,
    pub source: &'static str,
    pub kind: ModelKind,
}

const DEMUCS_SOURCE: &str = "HTDemucs v4 (Meta, MIT), ONNX export HRSadeghi/demucs-onnx @ 9128f6e (its card calls the weights non-commercial; the Demucs LICENSE it links is MIT)";
const WHISPER_SOURCE: &str = "OpenAI Whisper (MIT), ggml conversion ggerganov/whisper.cpp @ 5359861";

macro_rules! whisper {
    ($id:literal, $file:literal, $sha:literal, $bytes:literal) => {
        ModelInfo {
            id: $id,
            file: $file,
            url: concat!("https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/", $file),
            sha256: $sha,
            bytes: $bytes,
            licence: "MIT",
            source: WHISPER_SOURCE,
            kind: ModelKind::Transcription,
        }
    };
}

/// Every model WaveCraft knows. Only the ones a user (or an agent with `download: true`) asks for
/// are ever downloaded.
pub const MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "htdemucs",
        file: "htdemucs.onnx",
        url: concat!("https://huggingface.co/HRSadeghi/demucs-onnx/resolve/9128f6ee0f1b8e333b5b8d12613aefb3bebc249a/", "htdemucs.onnx"),
        sha256: "abaac48bc8fd55fa672268c8f13228b9b00f52f0c4cd10945048982eca4643bc",
        bytes: 168_683_222,
        licence: "MIT",
        source: DEMUCS_SOURCE,
        kind: ModelKind::Separation { stems: 4 },
    },
    ModelInfo {
        id: "htdemucs_6s",
        file: "htdemucs_6s.onnx",
        url: concat!("https://huggingface.co/HRSadeghi/demucs-onnx/resolve/9128f6ee0f1b8e333b5b8d12613aefb3bebc249a/", "htdemucs_6s.onnx"),
        sha256: "8bf891f10eb208f6fd945f427c8e348ddddef38069d47b8afb7d4aabc24bc15f",
        bytes: 110_396_023,
        licence: "MIT",
        source: DEMUCS_SOURCE,
        kind: ModelKind::Separation { stems: 6 },
    },
    whisper!("whisper-tiny", "ggml-tiny.bin", "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21", 77_691_713),
    whisper!("whisper-base", "ggml-base.bin", "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe", 147_951_465),
    whisper!("whisper-small", "ggml-small.bin", "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b", 487_601_967),
    whisper!("whisper-medium", "ggml-medium.bin", "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208", 1_533_763_059),
    whisper!("whisper-large-v3-turbo", "ggml-large-v3-turbo.bin", "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69", 1_624_555_275),
    whisper!(
        "whisper-large-v3-turbo-q5_0",
        "ggml-large-v3-turbo-q5_0.bin",
        "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        574_041_195
    ),
    whisper!(
        "whisper-large-v3-turbo-q8_0",
        "ggml-large-v3-turbo-q8_0.bin",
        "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
        874_188_075
    ),
];

/// The owner's first choice (design, "Settled at the start of F2").
pub const DEFAULT_WHISPER: &str = "whisper-large-v3-turbo-q5_0";

/// The registry entry `id`.
pub fn model(id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == id)
}

/// One separated source: its name ("vocals", "drums"…) and its channels.
#[derive(Debug, Clone, PartialEq)]
pub struct Stem {
    pub name: String,
    pub channels: Vec<Vec<f32>>,
}

/// The model runtime the engine calls. Errors are messages for the user.
pub trait Models: Send + Sync {
    /// Where model `id` is, when it is installed (present with the right size).
    fn installed(&self, id: &str) -> Option<PathBuf>;
    /// Model `id` checked by size and sha256, downloaded when missing or damaged;
    /// `progress(done, total)` in bytes returns false to cancel.
    fn download(&self, id: &str, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<PathBuf, String>;
    /// Separate `channels` (at `sample_rate`) with separation model `id`: stems of the same length,
    /// rate and channel count; `progress` from 0 to 1 returns false to cancel.
    fn separate(&self, id: &str, channels: &[Vec<f32>], sample_rate: u32, progress: &mut dyn FnMut(f32) -> bool) -> Result<Vec<Stem>, String>;
    /// The words in `mono_16k` (16 kHz mono) with their times in seconds; `language` "it", "en", or
    /// `None` to detect it.
    fn transcribe(&self, id: &str, mono_16k: &[f32], language: Option<&str>) -> Result<Vec<Word>, String>;
    /// `channels` played `tempo` times as fast (0.5–1.5) and moved by `semitones`: the length becomes
    /// `round(len / tempo)`.
    fn stretch(&self, channels: &[Vec<f32>], sample_rate: u32, tempo: f32, semitones: f32) -> Result<Vec<Vec<f32>>, String>;
}
