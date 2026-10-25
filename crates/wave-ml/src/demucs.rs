//! HTDemucs on ONNX Runtime: the network of the host side in `wave_analysis::demucs`.

use ort::session::Session;
use ort::value::Tensor;
use soundcraft_wave_analysis::demucs::{self as host, BINS, FRAMES, RATE, SEGMENT};
use soundcraft_wave_analysis::models::Stem;
use soundcraft_wave_analysis::resample;
use std::path::Path;
use std::sync::OnceLock;

const NAMES: [&str; 6] = ["drums", "bass", "other", "vocals", "guitar", "piano"];

fn environment() -> Result<&'static ort::environment::Environment, String> {
    static ENV: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| ort::environment::init().with_telemetry(false).build().map_err(|e| format!("ONNX Runtime: {e}")))
        .as_ref()
        .map_err(Clone::clone)
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

fn session(path: &Path, coreml: bool) -> Result<Session, String> {
    let env = environment()?;
    let mut builder = Session::builder(env).map_err(|e| format!("ONNX Runtime: {e}"))?;
    if coreml {
        builder = builder.with_execution_providers([ort::ep::CoreML::default().build()]).map_err(|e| format!("ONNX Runtime (CoreML): {e}"))?;
    }
    builder
        .with_intra_threads(threads())
        .map_err(|e| format!("ONNX Runtime: {e}"))?
        .commit_from_file(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Exactly `len` samples of `x` (resampling may round one off).
fn fit(mut x: Vec<f32>, len: usize) -> Vec<f32> {
    x.resize(len, 0.0);
    x
}

/// Separate `channels` (mono or stereo at `sample_rate`) into `stems` stems with the model at `path`.
pub fn separate(
    path: &Path,
    coreml: bool,
    stems: usize,
    channels: &[Vec<f32>],
    sample_rate: u32,
    progress: &mut dyn FnMut(f32),
) -> Result<Vec<Stem>, String> {
    let (Some(left), right) = (channels.first(), channels.get(1)) else {
        return Err("no audio to separate".into());
    };
    if channels.len() > 2 {
        return Err("separation takes mono or stereo audio".into());
    }
    if sample_rate == 0 {
        return Err("the sample rate must be positive".into());
    }
    let len = left.len();
    let to44 = |x: &[f32]| if sample_rate == RATE { x.to_vec() } else { resample(x, sample_rate, RATE) };
    let l = to44(left);
    let r = right.map_or_else(|| l.clone(), |r| to44(r));
    let n44 = l.len().min(r.len());
    let stereo = [fit(l, n44), fit(r, n44)];
    let mut s = session(path, coreml)?;
    let outputs: Vec<String> = s.outputs().iter().map(|o| o.name().to_string()).collect();
    let (spec_name, wav_name) = match outputs.as_slice() {
        [a, b] => (a.clone(), b.clone()),
        _ => return Err(format!("{}: expected a graph with two outputs, found {}", path.display(), outputs.len())),
    };
    let mut net = |mix: &[f32], mag: &[f32]| -> Result<(Vec<f32>, Vec<f32>), String> {
        let mix = Tensor::from_array(([1usize, 2, SEGMENT], mix.to_vec())).map_err(|e| e.to_string())?;
        let mag = Tensor::from_array(([1usize, 4, BINS, FRAMES], mag.to_vec())).map_err(|e| e.to_string())?;
        let out = s.run(ort::inputs![mix, mag]).map_err(|e| format!("separation failed: {e}"))?;
        let take = |name: &str| -> Result<Vec<f32>, String> {
            let v = out.get(name).ok_or_else(|| format!("the graph has no output `{name}`"))?;
            let (_, data) = v.try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            if data.iter().any(|x| !x.is_finite()) {
                return Err("the separation network returned non-finite values".into());
            }
            Ok(data.to_vec())
        };
        Ok((take(&spec_name)?, take(&wav_name)?))
    };
    let separated = host::separate_with(&stereo, stems, &mut net, progress)?;
    let back = |x: &[f32]| fit(if sample_rate == RATE { x.to_vec() } else { resample(x, RATE, sample_rate) }, len);
    Ok(separated
        .iter()
        .enumerate()
        .map(|(i, [sl, sr])| {
            let channels = if right.is_some() {
                vec![back(sl), back(sr)]
            } else {
                let mono: Vec<f32> = sl.iter().zip(sr).map(|(a, b)| 0.5 * (a + b)).collect();
                vec![back(&mono)]
            };
            Stem { name: NAMES.get(i).copied().unwrap_or("other").to_string(), channels }
        })
        .collect())
}
