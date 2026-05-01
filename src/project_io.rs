//! Load and save projects as `project.json` plus one JSON file per track under `tracks/`.
//!
//! Notes in track files use a compact MIDI-style encoding: each note is
//! `[pitch, velocity_midi, start_beat, length_beats]` (see [`crate::project::MidiNote`]).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::project::{Project, Track};

pub const PROJECT_MANIFEST: &str = "project.json";
pub const PROJECT_SCHEMA_V1: &str = "music-rs-project/v1";

#[derive(Deserialize)]
struct ProjectManifest {
    schema: String,
    name: String,
    tempo_bpm: f32,
    loop_start_bar: u32,
    loop_bars: u32,
    master_gain: f32,
    tracks: Vec<String>,
}

/// True if `path` is a `.../project.json` file path (split layout manifest).
pub fn is_split_manifest_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == PROJECT_MANIFEST)
}

/// Resolve a project directory or manifest file to the manifest path.
pub fn resolve_manifest_path(path: &Path) -> Result<PathBuf> {
    if path.is_dir() {
        return Ok(path.join(PROJECT_MANIFEST));
    }
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == PROJECT_MANIFEST)
    {
        return Ok(path.to_path_buf());
    }
    anyhow::bail!(
        "expected a project folder or a file named {}; got {}",
        PROJECT_MANIFEST,
        path.display()
    );
}

/// Load from a project directory (containing `project.json`) or from a `project.json` path.
pub fn load_project(path: &Path) -> Result<Project> {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi") {
            anyhow::bail!(
                "MIDI is not a project format here; convert first:\n  daw midi-to-json <input.mid> <output_dir_or_.../project.json>\nThen open the project folder or its project.json."
            );
        }
    }

    let manifest_path = resolve_manifest_path(path).with_context(|| path.display().to_string())?;

    let base_dir = manifest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("loading {}", manifest_path.display()))?;

    let envelope: ProjectManifest =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", manifest_path.display()))?;

    if envelope.schema != PROJECT_SCHEMA_V1 {
        anyhow::bail!(
            "unsupported project schema {:?} (expected {:?})",
            envelope.schema,
            PROJECT_SCHEMA_V1
        );
    }

    let mut out = Vec::with_capacity(envelope.tracks.len());
    for rel in &envelope.tracks {
        let track_path = base_dir.join(rel);
        let track_json = std::fs::read_to_string(&track_path).with_context(|| {
            format!(
                "loading track file {} (referenced from {})",
                track_path.display(),
                manifest_path.display()
            )
        })?;
        let track: Track = serde_json::from_str(&track_json)
            .with_context(|| format!("parsing track {}", track_path.display()))?;
        out.push(track);
    }

    let mut project = Project {
        name: envelope.name,
        tempo_bpm: envelope.tempo_bpm,
        loop_start_bar: envelope.loop_start_bar,
        loop_bars: envelope.loop_bars,
        master_gain: envelope.master_gain,
        tracks: out,
    };
    project.clamp_settings();
    Ok(project)
}

fn slugify_track_file_stem(raw: &str) -> String {
    let slug: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .fold(String::new(), |mut acc, c| {
            if c == '_' && acc.ends_with('_') {
                return acc;
            }
            acc.push(c);
            acc
        });
    let slug = slug.trim_matches('_');
    if slug.is_empty() || slug.chars().all(|c| c == '_') {
        return String::new();
    }
    truncate_utf8_stem(slug, 56)
}

fn truncate_utf8_stem(s: &str, max_bytes: usize) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if out.len() + ch.len_utf8() > max_bytes {
            break;
        }
        out.push(ch);
    }
    out.trim_matches('_').to_string()
}

fn track_file_name(index: usize, track: &Track) -> String {
    let mut slug = slugify_track_file_stem(&track.name);
    if slug.is_empty() {
        slug = slugify_track_file_stem(&track.id);
    }
    let slug = if slug.is_empty() {
        format!("track_{index:03}")
    } else {
        slug
    };
    format!("{index:03}_{slug}.json")
}

/// Save split layout to `manifest_path` (must be `.../project.json`).
pub fn save_project(project: &Project, manifest_path: &Path) -> Result<()> {
    if !is_split_manifest_path(manifest_path) {
        anyhow::bail!(
            "save path must end with {} (split project layout is the only supported format)",
            PROJECT_MANIFEST
        );
    }
    let base = manifest_path
        .parent()
        .context("project.json path must live inside a project directory")?;
    let tracks_dir = base.join("tracks");
    std::fs::create_dir_all(&tracks_dir)
        .with_context(|| format!("creating {}", tracks_dir.display()))?;
    if tracks_dir.is_dir() {
        for entry in std::fs::read_dir(&tracks_dir)
            .with_context(|| format!("reading {}", tracks_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                std::fs::remove_file(&path)
                    .with_context(|| format!("removing stale {}", path.display()))?;
            }
        }
    }

    let mut refs = Vec::with_capacity(project.tracks.len());
    for (i, track) in project.tracks.iter().enumerate() {
        let fname = track_file_name(i, track);
        let rel = format!("tracks/{fname}");
        let full = tracks_dir.join(&fname);
        let body = serde_json::to_string_pretty(track)
            .with_context(|| format!("serializing track {}", track.name))?;
        std::fs::write(&full, body).with_context(|| format!("writing {}", full.display()))?;
        refs.push(rel);
    }

    #[derive(serde::Serialize)]
    struct SplitManifest<'a> {
        schema: &'static str,
        name: &'a str,
        tempo_bpm: f32,
        loop_start_bar: u32,
        loop_bars: u32,
        master_gain: f32,
        tracks: &'a [String],
    }

    let manifest = SplitManifest {
        schema: PROJECT_SCHEMA_V1,
        name: &project.name,
        tempo_bpm: project.tempo_bpm,
        loop_start_bar: project.loop_start_bar,
        loop_bars: project.loop_bars,
        master_gain: project.master_gain,
        tracks: &refs,
    };
    let json = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(manifest_path, json)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Instrument, MidiNote, Waveform};

    #[test]
    fn rejects_midi_extension_with_hint() {
        let dir = std::env::temp_dir().join(format!(
            "music-rs-midi-reject-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mid = dir.join("test.mid");
        std::fs::write(&mid, b"MThd").unwrap();
        let err = load_project(&mid).unwrap_err().to_string();
        assert!(err.contains("midi-to-json"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_bundled_example_project() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/projects/happy_birthday");
        let loaded = load_project(&sample).unwrap();
        assert_eq!(loaded.name, "Happy Birthday");
        assert_eq!(loaded.tracks.len(), 4);
        assert!(loaded.tracks.iter().any(|t| !t.clips.is_empty()));
    }

    #[test]
    fn split_round_trip_preserves_note_tuples() {
        let dir = std::env::temp_dir().join(format!(
            "music-rs-split-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join(PROJECT_MANIFEST);

        let mut project = Project::blank();
        project.name = "Split Test".to_owned();
        project.tempo_bpm = 99.0;
        let tid = project.create_track(
            "A",
            Instrument::Synth {
                waveform: Waveform::Sine,
            },
        );
        project.add_clip(&tid, "Clip", 0.0, 4.0);
        if let Some(track) = project.track_mut(&tid) {
            if let Some(clip) = track.clips.first_mut() {
                clip.add_note(MidiNote {
                    pitch: 60,
                    velocity: 0.5,
                    start_beat: 0.0,
                    length_beats: 0.25,
                });
            }
        }

        save_project(&project, &manifest).unwrap();
        let loaded = load_project(&dir).unwrap();
        assert_eq!(loaded.name, project.name);
        assert_eq!(loaded.tempo_bpm, project.tempo_bpm);
        assert_eq!(loaded.tracks.len(), 1);
        assert_eq!(loaded.tracks[0].clips[0].notes.len(), 1);
        let n = &loaded.tracks[0].clips[0].notes[0];
        assert_eq!(n.pitch, 60);
        assert!((n.velocity - 0.5).abs() < 0.02);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_names_track_files_from_track_title() {
        let dir = std::env::temp_dir().join(format!(
            "music-rs-track-names-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join(PROJECT_MANIFEST);

        let mut project = Project::blank();
        project.name = "Name Test".to_owned();
        project.create_track("Drum Bus", Instrument::DrumSampler);

        save_project(&project, &manifest).unwrap();
        let text = std::fs::read_to_string(&manifest).unwrap();
        assert!(
            text.contains("000_drum_bus.json"),
            "manifest should reference slug from track name: {text}"
        );
        assert!(dir.join("tracks/000_drum_bus.json").is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
