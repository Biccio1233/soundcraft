//! Long jobs — analysis, model downloads, separation, transcription, practice renders — run on a
//! worker thread; the window stays live, the status line shows the progress, and the result is
//! applied on the UI thread (refused when the file changed meanwhile). One job at a time; Cancel
//! (`wave.cancel_job`) drops it at once and the worker stops at its next progress report.

use crate::WaveApp;
use serde_json::{Value, json};
use soundcraft_engine::cmd::{wave_analysis as an, wave_ml as ml};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

pub enum Done {
    Analysis(an::AnalysisDone),
    Download(Result<Value, String>),
    Separation(Result<ml::SeparationDone, String>),
    Transcription(Result<ml::TranscriptionDone, String>),
    Practice(Result<ml::PracticeDone, String>),
}

pub struct Job {
    progress: Arc<Mutex<(f32, String)>>,
    cancel: Arc<AtomicBool>,
    rx: Receiver<Done>,
}

impl WaveApp {
    pub fn job_running(&self) -> bool {
        self.job.is_some()
    }

    /// "Separating (htdemucs) 42 %" while a job runs.
    pub fn job_status(&self) -> Option<String> {
        let job = self.job.as_ref()?;
        let (f, label) = job.progress.lock().map(|g| g.clone()).unwrap_or_default();
        Some(format!("{label}… {:.0} %", (f * 100.0).clamp(0.0, 100.0)))
    }

    /// Stop the running job: it is dropped now, its worker stops at its next progress report and
    /// its result is never applied.
    pub fn cancel_job(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
            self.status = "Cancelled".into();
        }
    }

    fn start_job(&mut self, label: &str, work: impl FnOnce(&mut dyn FnMut(f32, &str) -> bool) -> Done + Send + 'static) {
        if self.job.is_some() {
            self.status = "a job is already running".into();
            return;
        }
        let (tx, rx) = channel();
        let progress = Arc::new(Mutex::new((0.0f32, label.to_string())));
        let cancel = Arc::new(AtomicBool::new(false));
        let (shared, stop) = (Arc::clone(&progress), Arc::clone(&cancel));
        let spawned = std::thread::Builder::new().name("wavecraft-job".into()).spawn(move || {
            let mut report = |f: f32, l: &str| {
                if let Ok(mut g) = shared.lock() {
                    *g = (f, l.to_string());
                }
                !stop.load(Ordering::Relaxed)
            };
            let _ = tx.send(work(&mut report));
        });
        match spawned {
            Ok(_) => self.job = Some(Job { progress, cancel, rx }),
            Err(e) => self.status = format!("could not start the job: {e}"),
        }
    }

    /// Apply the job's result when it is ready (called every frame).
    pub(crate) fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        match job.rx.try_recv() {
            Ok(done) => {
                self.job = None;
                self.apply_done(done);
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            Err(TryRecvError::Disconnected) => {
                self.job = None;
                self.status = "the job stopped unexpectedly".into();
            }
        }
    }

    fn apply_done(&mut self, done: Done) {
        let separated = matches!(done, Done::Separation(Ok(_)));
        let e = &mut self.engine;
        let result: Result<(Value, &str), String> = match done {
            Done::Analysis(d) => an::apply_analysis(e, d).map(|v| (v, "Analysis done")).map_err(|x| x.to_string()),
            Done::Download(r) => r.map(|v| (v, "Model downloaded")),
            Done::Separation(r) => r.and_then(|d| ml::apply_separation(e, d).map_err(|x| x.to_string())).map(|v| (v, "Stems written")),
            Done::Transcription(r) => r.and_then(|d| ml::apply_transcription(e, d).map_err(|x| x.to_string())).map(|v| (v, "Lyrics transcribed")),
            Done::Practice(r) => r.and_then(|d| ml::apply_practice(e, d).map_err(|x| x.to_string())).map(|v| (v, "Practice ready")),
        };
        let reanalyze = separated && result.is_ok();
        self.status = match result {
            Ok((v, what)) => match v.get("words").and_then(Value::as_u64) {
                Some(n) => format!("{what}: {n} words"),
                None => what.to_string(),
            },
            Err(err) => err,
        };
        // Chords and key now come from the stems: the lanes on screen are analysed again.
        if reanalyze {
            crate::analysis::reanalyze(self);
        }
    }

    /// `wave.analyze` on the selection (or the whole file), in the background.
    pub fn analyze_in_background(&mut self) {
        self.analyze_in_background_with(&json!({}));
    }

    /// `wave.analyze` with `params`, in the background.
    pub(crate) fn analyze_in_background_with(&mut self, params: &Value) {
        match an::analysis_job(&self.engine, params) {
            Ok(job) => self.start_job("Analyzing", move |report| {
                let _ = report(0.0, "Analyzing");
                Done::Analysis(job.run())
            }),
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn download_in_background(&mut self, id: &str) {
        match ml::download_job(&self.engine, &json!({"id": id})) {
            Ok(job) => self.start_job(&format!("Downloading {id}"), move |report| Done::Download(job.run(report))),
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn separate_in_background(&mut self, params: Value) {
        match ml::separation_job(&self.engine, &params) {
            Ok(job) => self.start_job("Separating", move |report| Done::Separation(job.run(report))),
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn transcribe_in_background(&mut self, params: Value) {
        match ml::transcription_job(&self.engine, &params) {
            Ok(job) => self.start_job("Transcribing", move |report| Done::Transcription(job.run(report))),
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn practice_in_background(&mut self, params: Value) {
        match ml::practice_job(&self.engine, &params) {
            Ok(job) => self.start_job("Rendering practice", move |report| Done::Practice(job.run(report))),
            Err(err) => self.status = err.to_string(),
        }
    }
}
