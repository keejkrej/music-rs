use anyhow::Result;

use crate::{
    commands::{DrumStyle, EditCommand, bassline, chord_stabs, validate_commands},
    project::{Instrument, Project, Waveform},
};

#[derive(Debug, Clone)]
pub struct AiRequest {
    pub prompt: String,
    pub project_summary: String,
}

pub trait AiProvider {
    fn propose_edits(&self, request: &AiRequest, project: &Project) -> Result<Vec<EditCommand>>;
}

#[derive(Debug, Default)]
pub struct RuleBasedProvider;

impl AiProvider for RuleBasedProvider {
    fn propose_edits(&self, request: &AiRequest, project: &Project) -> Result<Vec<EditCommand>> {
        let prompt = request.prompt.to_lowercase();
        let bars = parse_bars(&prompt)
            .unwrap_or(project.loop_bars)
            .clamp(4, 16);
        let mut commands = Vec::new();

        if let Some(bpm) = parse_bpm(&prompt) {
            commands.push(EditCommand::SetTempo { bpm });
        }
        if prompt.contains("house") || prompt.contains("drum") || prompt.contains("loop") {
            commands.push(EditCommand::ArrangeLoop { bars });
            let drum_track = project
                .tracks
                .iter()
                .find(|track| matches!(track.instrument, Instrument::DrumSampler))
                .map(|track| track.id.clone());
            if drum_track.is_none() {
                commands.push(EditCommand::CreateTrack {
                    name: "Drums".to_owned(),
                    instrument: Instrument::DrumSampler,
                });
            }
            commands.push(EditCommand::MakeDrumPattern {
                track_id: drum_track.or_else(|| Some("__newest__".to_owned())),
                bars,
                style: if prompt.contains("minimal") {
                    DrumStyle::Minimal
                } else {
                    DrumStyle::House
                },
            });
        }
        if prompt.contains("bass") || prompt.contains("dark") || prompt.contains("darker") {
            let bass_track = project
                .tracks
                .iter()
                .find(|track| track.name.to_lowercase().contains("bass"))
                .map(|track| track.id.clone());
            if bass_track.is_none() {
                commands.push(EditCommand::CreateTrack {
                    name: "Bass".to_owned(),
                    instrument: Instrument::Synth {
                        waveform: Waveform::Square,
                    },
                });
            }
            let target = bass_track.unwrap_or_else(|| "__newest__".to_owned());
            commands.push(EditCommand::AddNotes {
                track_id: target,
                clip_id: None,
                notes: bassline(43, bars, prompt.contains("dark")),
            });
        } else if prompt.contains("chord") || prompt.contains("pad") || prompt.contains("idea") {
            let target = project
                .tracks
                .iter()
                .find(|track| matches!(track.instrument, Instrument::Synth { .. }))
                .map(|track| track.id.clone())
                .unwrap_or_else(|| "__newest__".to_owned());
            if target == "__newest__" {
                commands.push(EditCommand::CreateTrack {
                    name: "Chords".to_owned(),
                    instrument: Instrument::Synth {
                        waveform: Waveform::Saw,
                    },
                });
            }
            commands.push(EditCommand::AddNotes {
                track_id: target,
                clip_id: None,
                notes: chord_stabs(48, bars),
            });
        }

        if commands.is_empty() {
            commands.push(EditCommand::ArrangeLoop { bars });
            let drum_track = project
                .tracks
                .iter()
                .find(|track| matches!(track.instrument, Instrument::DrumSampler))
                .map(|track| track.id.clone());
            if drum_track.is_none() {
                commands.push(EditCommand::CreateTrack {
                    name: "Drums".to_owned(),
                    instrument: Instrument::DrumSampler,
                });
            }
            commands.push(EditCommand::MakeDrumPattern {
                track_id: drum_track.or_else(|| Some("__newest__".to_owned())),
                bars,
                style: DrumStyle::Minimal,
            });
        }

        Ok(commands)
    }
}

pub fn validated_ai_edits(
    provider: &dyn AiProvider,
    prompt: impl Into<String>,
    project: &Project,
) -> Result<Vec<EditCommand>> {
    let request = AiRequest {
        prompt: prompt.into(),
        project_summary: project.compact_summary(),
    };
    let commands = provider.propose_edits(&request, project)?;
    validate_commands(project, &commands)?;
    Ok(commands)
}

fn parse_bpm(prompt: &str) -> Option<f32> {
    let words: Vec<&str> = prompt.split_whitespace().collect();
    for window in words.windows(2) {
        if window[1] == "bpm" {
            if let Ok(value) = window[0].parse::<f32>() {
                return Some(value);
            }
        }
    }
    None
}

fn parse_bars(prompt: &str) -> Option<u32> {
    let words: Vec<&str> = prompt.split_whitespace().collect();
    for window in words.windows(2) {
        if window[1].starts_with("bar") {
            if let Ok(value) = window[0].parse::<u32>() {
                return Some(value);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_house_prompt_returns_valid_commands() {
        let project = Project::default();
        let edits = validated_ai_edits(
            &RuleBasedProvider,
            "make an 8 bar house loop at 124 BPM",
            &project,
        )
        .unwrap();
        assert!(
            edits
                .iter()
                .any(|edit| matches!(edit, EditCommand::SetTempo { bpm } if *bpm == 124.0))
        );
        assert!(
            edits
                .iter()
                .any(|edit| matches!(edit, EditCommand::MakeDrumPattern { bars: 8, .. }))
        );
    }
}
