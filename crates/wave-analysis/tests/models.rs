use soundcraft_wave_analysis::models::{DEFAULT_WHISPER, MODELS, ModelKind, model};

#[test]
fn registry_is_pinned() {
    let mut ids: Vec<&str> = MODELS.iter().map(|m| m.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), MODELS.len(), "duplicate ids");
    for m in MODELS {
        assert!(m.sha256.len() == 64 && m.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{}: sha256", m.id);
        assert!(m.bytes > 0, "{}: size", m.id);
        let rev = m.url.split("/resolve/").nth(1).and_then(|r| r.split('/').next()).unwrap_or_default();
        assert!(
            m.url.starts_with("https://huggingface.co/") && rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit()),
            "{}: url {}",
            m.id,
            m.url
        );
        assert!(m.url.ends_with(m.file), "{}: file", m.id);
        assert!(!m.licence.is_empty() && !m.source.is_empty(), "{}: provenance", m.id);
    }
    assert!(matches!(model(DEFAULT_WHISPER).map(|m| m.kind), Some(ModelKind::Transcription)));
    assert!(matches!(model("htdemucs").map(|m| m.kind), Some(ModelKind::Separation { stems: 4 })));
    assert!(matches!(model("htdemucs_6s").map(|m| m.kind), Some(ModelKind::Separation { stems: 6 })));
    assert_eq!(model("htdemucs").map(|m| m.bytes), Some(168_683_222));
    assert_eq!(model(DEFAULT_WHISPER).map(|m| m.sha256), Some("394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2"));
    assert!(model("nope").is_none());
}
