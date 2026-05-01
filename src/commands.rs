use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::project::{
    BEATS_PER_BAR, Clip, Instrument, MAX_LOOP_BARS, MIN_LOOP_BARS, MidiNote, Project, Waveform,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum EditCommand {
    CreateTrack {
        name: String,
        instrument: Instrument,
    },
    AddNotes {
        track_id: String,
        clip_id: Option<String>,
        notes: Vec<MidiNote>,
    },
    ReplaceClip {
        track_id: String,
        clip: Clip,
    },
    SetTempo {
        bpm: f32,
    },
    MakeDrumPattern {
        track_id: Option<String>,
        bars: u32,
        style: DrumStyle,
    },
    ArrangeLoop {
        bars: u32,
    },
    SetMixer {
        track_id: String,
        gain: Option<f32>,
        pan: Option<f32>,
        mute: Option<bool>,
        solo: Option<bool>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DrumStyle {
    House,
    Trap,
    Rock,
    Minimal,
}

pub fn validate_commands(project: &Project, commands: &[EditCommand]) -> Result<()> {
    let mut scratch = project.clone();
    apply_commands(&mut scratch, commands.to_vec())?;
    Ok(())
}

pub fn apply_commands(project: &mut Project, commands: Vec<EditCommand>) -> Result<()> {
    for command in commands {
        let command = resolve_batch_reference(project, command)?;
        apply_command(project, command)?;
    }
    project.clamp_settings();
    Ok(())
}

fn resolve_batch_reference(project: &Project, command: EditCommand) -> Result<EditCommand> {
    match command {
        EditCommand::AddNotes {
            track_id,
            clip_id,
            notes,
        } if track_id == "__newest__" => {
            let track_id = project
                .tracks
                .last()
                .map(|track| track.id.clone())
                .ok_or_else(|| anyhow!("cannot resolve __newest__ without any tracks"))?;
            Ok(EditCommand::AddNotes {
                track_id,
                clip_id,
                notes,
            })
        }
        EditCommand::SetMixer {
            track_id,
            gain,
            pan,
            mute,
            solo,
        } if track_id == "__newest__" => {
            let track_id = project
                .tracks
                .last()
                .map(|track| track.id.clone())
                .ok_or_else(|| anyhow!("cannot resolve __newest__ without any tracks"))?;
            Ok(EditCommand::SetMixer {
                track_id,
                gain,
                pan,
                mute,
                solo,
            })
        }
        EditCommand::MakeDrumPattern {
            track_id: Some(track_id),
            bars,
            style,
        } if track_id == "__newest__" => {
            let track_id = project
                .tracks
                .last()
                .map(|track| track.id.clone())
                .ok_or_else(|| anyhow!("cannot resolve __newest__ without any tracks"))?;
            Ok(EditCommand::MakeDrumPattern {
                track_id: Some(track_id),
                bars,
                style,
            })
        }
        other => Ok(other),
    }
}

pub fn apply_command(project: &mut Project, command: EditCommand) -> Result<()> {
    match command {
        EditCommand::CreateTrack { name, instrument } => {
            if name.trim().is_empty() {
                bail!("track name cannot be empty");
            }
            project.create_track(name, instrument);
        }
        EditCommand::AddNotes {
            track_id,
            clip_id,
            notes,
        } => {
            validate_notes(&notes)?;
            let loop_len = project.loop_length_beats();
            let track = project
                .track_mut(&track_id)
                .with_context(|| format!("track {track_id} does not exist"))?;
            let clip = if let Some(clip_id) = clip_id {
                track
                    .clips
                    .iter_mut()
                    .find(|clip| clip.id == clip_id)
                    .with_context(|| format!("clip {clip_id} does not exist"))?
            } else {
                if track.clips.is_empty() {
                    track.clips.push(Clip {
                        id: crate::project::new_id(),
                        name: "Main".to_owned(),
                        start_beat: 0.0,
                        length_beats: loop_len,
                        notes: Vec::new(),
                    });
                }
                track.clips.first_mut().unwrap()
            };
            for note in notes {
                if note.start_beat + note.length_beats > clip.length_beats + 0.001 {
                    bail!("note exceeds target clip length");
                }
                clip.add_note(note);
            }
        }
        EditCommand::ReplaceClip { track_id, clip } => {
            if clip.length_beats <= 0.0 {
                bail!("clip length must be positive");
            }
            validate_notes(&clip.notes)?;
            let track = project
                .track_mut(&track_id)
                .with_context(|| format!("track {track_id} does not exist"))?;
            if let Some(existing) = track
                .clips
                .iter_mut()
                .find(|existing| existing.id == clip.id)
            {
                *existing = clip;
            } else {
                track.clips.push(clip);
            }
        }
        EditCommand::SetTempo { bpm } => {
            if !(40.0..=240.0).contains(&bpm) {
                bail!("tempo must be between 40 and 240 BPM");
            }
            project.tempo_bpm = bpm;
        }
        EditCommand::MakeDrumPattern {
            track_id,
            bars,
            style,
        } => {
            if !(MIN_LOOP_BARS..=MAX_LOOP_BARS).contains(&bars) {
                bail!("drum pattern must be 4-16 bars");
            }
            let track_id = match track_id {
                Some(track_id) => track_id,
                None => project
                    .first_track_id_for(|instrument| matches!(instrument, Instrument::DrumSampler))
                    .ok_or_else(|| anyhow!("no drum track exists"))?,
            };
            let notes = drum_notes(bars, style);
            let clip_id = project
                .track(&track_id)
                .and_then(|track| track.clips.first())
                .map(|clip| clip.id.clone())
                .unwrap_or_else(crate::project::new_id);
            let clip = Clip {
                id: clip_id,
                name: format!("{style:?} Pattern"),
                start_beat: 0.0,
                length_beats: bars as f32 * BEATS_PER_BAR,
                notes,
            };
            apply_command(project, EditCommand::ReplaceClip { track_id, clip })?;
        }
        EditCommand::ArrangeLoop { bars } => {
            if !(MIN_LOOP_BARS..=MAX_LOOP_BARS).contains(&bars) {
                bail!("loop must be 4-16 bars");
            }
            project.loop_bars = bars;
            let loop_len = project.loop_length_beats();
            for track in &mut project.tracks {
                if track.clips.is_empty() {
                    track.clips.push(Clip {
                        id: crate::project::new_id(),
                        name: "Main".to_owned(),
                        start_beat: 0.0,
                        length_beats: loop_len,
                        notes: Vec::new(),
                    });
                }
                for clip in &mut track.clips {
                    if clip.start_beat == 0.0 {
                        clip.length_beats = loop_len;
                    }
                }
            }
        }
        EditCommand::SetMixer {
            track_id,
            gain,
            pan,
            mute,
            solo,
        } => {
            let track = project
                .track_mut(&track_id)
                .with_context(|| format!("track {track_id} does not exist"))?;
            if let Some(gain) = gain {
                if !(0.0..=1.5).contains(&gain) {
                    bail!("gain must be between 0.0 and 1.5");
                }
                track.gain = gain;
            }
            if let Some(pan) = pan {
                if !(-1.0..=1.0).contains(&pan) {
                    bail!("pan must be between -1.0 and 1.0");
                }
                track.pan = pan;
            }
            if let Some(mute) = mute {
                track.mute = mute;
            }
            if let Some(solo) = solo {
                track.solo = solo;
            }
        }
    }
    Ok(())
}

pub fn bassline(root_pitch: u8, bars: u32, darker: bool) -> Vec<MidiNote> {
    let pattern = if darker {
        [0, -5, -2, -7]
    } else {
        [0, 3, 5, 7]
    };
    let mut notes = Vec::new();
    for bar in 0..bars {
        for step in 0..4 {
            let offset = pattern[(bar as usize + step) % pattern.len()];
            notes.push(MidiNote {
                pitch: (root_pitch as i16 + offset).clamp(0, 127) as u8,
                velocity: 0.72,
                start_beat: bar as f32 * BEATS_PER_BAR + step as f32,
                length_beats: 0.82,
            });
        }
    }
    notes
}

pub fn chord_stabs(root_pitch: u8, bars: u32) -> Vec<MidiNote> {
    let chord = [0, 3, 7, 10];
    let mut notes = Vec::new();
    for bar in 0..bars {
        for beat in [1.0, 2.5] {
            for interval in chord {
                notes.push(MidiNote {
                    pitch: root_pitch + interval,
                    velocity: 0.48,
                    start_beat: bar as f32 * BEATS_PER_BAR + beat,
                    length_beats: 0.35,
                });
            }
        }
    }
    notes
}

fn drum_notes(bars: u32, style: DrumStyle) -> Vec<MidiNote> {
    let mut notes = Vec::new();
    for bar in 0..bars {
        let base = bar as f32 * BEATS_PER_BAR;
        match style {
            DrumStyle::House => {
                for beat in 0..4 {
                    notes.push(note(36, base + beat as f32, 0.92, 0.18));
                    notes.push(note(42, base + beat as f32 + 0.5, 0.55, 0.08));
                }
                notes.push(note(38, base + 1.0, 0.72, 0.14));
                notes.push(note(38, base + 3.0, 0.72, 0.14));
            }
            DrumStyle::Trap => {
                notes.extend([0.0, 0.75, 2.25, 3.25].map(|b| note(36, base + b, 0.86, 0.16)));
                notes.push(note(38, base + 2.0, 0.82, 0.16));
                for step in 0..8 {
                    notes.push(note(42, base + step as f32 * 0.5, 0.4, 0.06));
                }
            }
            DrumStyle::Rock => {
                notes.extend([0.0, 2.0].map(|b| note(36, base + b, 0.9, 0.18)));
                notes.extend([1.0, 3.0].map(|b| note(38, base + b, 0.8, 0.14)));
                for step in 0..8 {
                    notes.push(note(42, base + step as f32 * 0.5, 0.45, 0.08));
                }
            }
            DrumStyle::Minimal => {
                notes.extend([0.0, 2.0].map(|b| note(36, base + b, 0.75, 0.16)));
                notes.push(note(38, base + 3.0, 0.55, 0.12));
                notes.extend([1.5, 2.5, 3.5].map(|b| note(42, base + b, 0.35, 0.06)));
            }
        }
    }
    notes
}

fn note(pitch: u8, start_beat: f32, velocity: f32, length_beats: f32) -> MidiNote {
    MidiNote {
        pitch,
        velocity,
        start_beat,
        length_beats,
    }
}

fn validate_notes(notes: &[MidiNote]) -> Result<()> {
    for note in notes {
        if note.pitch > 127 {
            bail!("MIDI pitch must be 0-127");
        }
        if !(0.0..=1.0).contains(&note.velocity) {
            bail!("note velocity must be 0.0-1.0");
        }
        if note.start_beat < 0.0 || note.length_beats <= 0.0 {
            bail!("note timing must be non-negative with positive length");
        }
    }
    Ok(())
}

pub fn default_synth(name: &str) -> EditCommand {
    EditCommand::CreateTrack {
        name: name.to_owned(),
        instrument: Instrument::Synth {
            waveform: Waveform::Saw,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_tempo() {
        let project = Project::default();
        let err = validate_commands(&project, &[EditCommand::SetTempo { bpm: 400.0 }]).unwrap_err();
        assert!(err.to_string().contains("tempo"));
    }

    #[test]
    fn applies_drum_pattern() {
        let mut project = Project::default();
        let drum_id = project.create_track("Drums", Instrument::DrumSampler);
        apply_command(
            &mut project,
            EditCommand::MakeDrumPattern {
                track_id: Some(drum_id.clone()),
                bars: 4,
                style: DrumStyle::House,
            },
        )
        .unwrap();
        assert!(!project.track(&drum_id).unwrap().clips[0].notes.is_empty());
    }
}
