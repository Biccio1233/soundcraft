//! WaveCraft's model runtime: checked model downloads, HTDemucs on ONNX Runtime, whisper.cpp
//! transcription and Signalsmith Stretch, behind the `Models` trait of `soundcraft-wave-analysis`.
//! Only the WaveCraft apps link this crate; the engine sees the trait.

mod backend;
mod demucs;
pub mod store;
mod stretch;
mod whisper;

pub use backend::{Backend, missing};
