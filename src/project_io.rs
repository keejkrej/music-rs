//! Load and save projects as a single JSON file or as a folder (`project.json` + `tracks/*.json`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::project::{Project, Track};

pub const PROJECT_MANIFEST: &str = "project.json";

#[derive(Deserialize)]
#[serde(untagged)]
enum TracksField {
    Inline(Vec<Track>),
    External(Vec<String>),
}

#[derive(Deserialize)]
struct ProjectEnvelope {
    name: String,
    tempo_bpm: f32,
    loop_start_bar: u32,
    loop_bars: u32,
    master_gain: f32,
    tracks: TracksField,
}

/// True if saving to this path should write the split folder layout (`tracks/` + manifest).
pub fn is_split_manifest_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == PROJECT_MANIFEST)
}

pub fn resolve_manifest_path(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.join(PROJECT_MANIFEST)
    } else {
        path.to_path_buf()
    }
}

/// Load from a `.json` file (monolithic or split manifest) or from a project directory containing `project.json`.
pub fn load_project(path: &Path) -> Result<Project> {
    let manifest_path = resolve_manifest_path(path);
    if let Some(ext) = manifest_path.extension().and_then(|e| e.to_str()) {
        if ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi") {
            anyhow::bail!(
                "MIDI is not a project format here; convert first:\n  daw midi-to-json <input.mid> <output.json>\nThen open the resulting JSON or project folder."
            );
        }
    }
    let base_dir = manifest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("loading {}", manifest_path.display()))?;

    let envelope: ProjectEnvelope =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", manifest_path.display()))?;

    let tracks = match envelope.tracks {
        TracksField::Inline(t) => t,
        TracksField::External(refs) => {
            let mut out = Vec::with_capacity(refs.len());
            for rel in refs {
                let track_path = base_dir.join(&rel);
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
            out
        }
    };

    let mut project = Project {
        name: envelope.name,
        tempo_bpm: envelope.tempo_bpm,
        loop_start_bar: envelope.loop_start_bar,
        loop_bars: envelope.loop_bars,
        master_gain: envelope.master_gain,
        tracks,
    };
    project.clamp_settings();
    Ok(project)
}

fn track_file_name(index: usize, track: &Track) -> String {
    let slug: String = track
        .id
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
    let slug = if slug.is_empty() || slug.chars().all(|c| c == '_') {
        format!("track_{index:03}")
    } else {
        slug
    };
    format!("{index:03}_{slug}.json")
}

/// Save monolithic JSON, or split layout when `path` ends with `project.json`.
pub fn save_project(project: &Project, path: &Path) -> Result<()> {
    if is_split_manifest_path(path) {
        let base = path.parent().context("project path has no parent directory")?;
        let tracks_dir = base.join("tracks");
        std::fs::create_dir_all(&tracks_dir)
            .with_context(|| format!("creating {}", tracks_dir.display()))?;

        let mut refs = Vec::with_capacity(project.tracks.len());
        for (i, track) in project.tracks.iter().enumerate() {
            let fname = track_file_name(i, track);
            let rel = format!("tracks/{fname}");
            let full = tracks_dir.join(&fname);
            let body = serde_json::to_string_pretty(track)
                .with_context(|| format!("serializing track {}", track.name))?;
            std::fs::write(&full, body)
                .with_context(|| format!("writing {}", full.display()))?;
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
            schema: "music-rs-project/v1",
            name: &project.name,
            tempo_bpm: project.tempo_bpm,
            loop_start_bar: project.loop_start_bar,
            loop_bars: project.loop_bars,
            master_gain: project.master_gain,
            tracks: &refs,
        };
        let json = serde_json::to_string_pretty(&manifest)?;
        std::fs::write(path, json)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    } else {
        std::fs::write(path, project.to_json()?)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Instrument, Waveform};

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
        assert!(
            err.contains("midi-to-json"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_bundled_monolithic_example() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/projects/happy_birthday.json");
        let loaded = load_project(&sample).unwrap();
        assert_eq!(loaded.name, "Happy Birthday");
        assert!(!loaded.tracks.is_empty());
    }

    #[test]
    fn split_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "music-rs-split-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join(PROJECT_MANIFEST);

        let mut project = Project::blank();
        project.name = "Split Test".to_owned();
        project.tempo_bpm = 99.0;
        project.tracks.push(Track {
            id: "t_a".to_owned(),
            name: "A".to_owned(),
            instrument: Instrument::Synth {
                waveform: Waveform::Sine,
            },
            gain: 0.9,
            pan: 0.0,
            mute: false,
            solo: false,
            clips: vec![],
        });

        save_project(&project, &manifest).unwrap();
        let loaded = load_project(&dir).unwrap();
        assert_eq!(loaded.name, project.name);
        assert_eq!(loaded.tempo_bpm, project.tempo_bpm);
        assert_eq!(loaded.tracks.len(), 1);
        assert_eq!(loaded.tracks[0].id, "t_a");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
