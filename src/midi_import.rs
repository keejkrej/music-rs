//! Convert Standard MIDI Files (SMF) into a [`Project`] (one DAW track per SMF track with notes).

use std::path::Path;

use anyhow::{Context, Result};
use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

use crate::project::{
    BEATS_PER_BAR, Clip, Instrument, MAX_LOOP_BARS, MIN_LOOP_BARS, MidiNote, Project, Track,
    Waveform, new_id,
};

const DRUM_MIDI_CHANNEL: u8 = 9;

/// Import a `.mid` / `.midi` file into a new project (best-effort; see module docs).
pub fn import_midi_path(path: &Path) -> Result<Project> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    import_midi_bytes(&bytes, path.file_stem().and_then(|s| s.to_str()))
}

pub fn import_midi_bytes(bytes: &[u8], name_hint: Option<&str>) -> Result<Project> {
    let smf = Smf::parse(bytes).context("parse MIDI file")?;

    let ppq = match smf.header.timing {
        Timing::Metrical(ppq) => u32::from(ppq.as_int()).max(1),
        Timing::Timecode(_, _) => {
            anyhow::bail!("SMPTE frame-based MIDI timing is not supported; re-export with PPQ timing")
        }
    };

    let tempo_bpm = first_tempo_bpm(&smf).unwrap_or(120.0);
    let tick_to_beat = 1.0_f32 / ppq as f32;

    let stem = name_hint.unwrap_or("Imported MIDI");
    let mut project = Project {
        name: stem.to_owned(),
        tempo_bpm,
        loop_start_bar: 0,
        loop_bars: MIN_LOOP_BARS,
        master_gain: 0.85,
        tracks: Vec::new(),
    };

    for (idx, track) in smf.tracks.iter().enumerate() {
        let (track_name, notes, all_drums) = parse_smf_track(track, tick_to_beat);
        if notes.is_empty() {
            continue;
        }

        let display_name = track_name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("MIDI {}", idx + 1));

        let instrument = if all_drums {
            Instrument::DrumSampler
        } else {
            Instrument::Synth {
                waveform: Waveform::Saw,
            }
        };

        let clip_name = display_name.clone();

        let mut t = Track {
            id: new_id(),
            name: display_name,
            instrument,
            gain: 0.85,
            pan: 0.0,
            mute: false,
            solo: false,
            clips: Vec::new(),
        };

        let end_beat = notes
            .iter()
            .map(|n| n.start_beat + n.length_beats)
            .fold(0.0_f32, f32::max)
            .max(BEATS_PER_BAR);

        let clip_len = end_beat
            .max(project.loop_length_beats())
            .max(MIN_LOOP_BARS as f32 * BEATS_PER_BAR);

        let clip = Clip {
            id: new_id(),
            name: clip_name,
            start_beat: 0.0,
            length_beats: clip_len,
            notes,
        };
        t.clips.push(clip);
        project.tracks.push(t);
    }

    if project.tracks.is_empty() {
        anyhow::bail!("MIDI file has no note data in any track");
    }

    let max_end = project
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .flat_map(|c| &c.notes)
        .map(|n| n.start_beat + n.length_beats)
        .fold(0.0_f32, f32::max);

    let bars_needed = ((max_end / BEATS_PER_BAR).ceil() as u32).clamp(MIN_LOOP_BARS, MAX_LOOP_BARS);
    project.loop_bars = bars_needed;
    project.clamp_settings();
    Ok(project)
}

fn first_tempo_bpm(smf: &Smf<'_>) -> Option<f32> {
    for track in &smf.tracks {
        for ev in track {
            if let TrackEventKind::Meta(MetaMessage::Tempo(us_per_qn)) = ev.kind {
                let micros: u32 = us_per_qn.as_int();
                if micros > 0 {
                    return Some(60_000_000.0 / micros as f32);
                }
            }
        }
    }
    None
}

fn parse_smf_track(
    track: &[TrackEvent<'_>],
    tick_to_beat: f32,
) -> (Option<String>, Vec<MidiNote>, bool) {
    use std::collections::HashMap;

    let mut abs_tick: u64 = 0;
    let mut track_name: Option<String> = None;
    let mut held: HashMap<(u8, u8), (u64, u8)> = HashMap::new();
    let mut notes: Vec<MidiNote> = Vec::new();
    let mut note_channels: Vec<u8> = Vec::new();

    for ev in track {
        abs_tick = abs_tick.saturating_add(u32::from(ev.delta.as_int()) as u64);

        match ev.kind {
            TrackEventKind::Meta(MetaMessage::TrackName(bytes)) => {
                let s = lossy_name(bytes);
                if !s.is_empty() {
                    track_name = Some(s);
                }
            }
            TrackEventKind::Midi { channel, message } => {
                let ch = channel.as_int();
                match message {
                    MidiMessage::NoteOn { key, vel } => {
                        let k = key.as_int();
                        let v = vel.as_int();
                        if v == 0 {
                            finish_note(&mut held, ch, k, abs_tick, tick_to_beat, &mut notes, &mut note_channels);
                        } else {
                            held.insert((ch, k), (abs_tick, v));
                        }
                    }
                    MidiMessage::NoteOff { key, .. } => {
                        let k = key.as_int();
                        finish_note(&mut held, ch, k, abs_tick, tick_to_beat, &mut notes, &mut note_channels);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let drum_hits = note_channels.iter().filter(|&&c| c == DRUM_MIDI_CHANNEL).count();
    let all_drums = !note_channels.is_empty() && drum_hits == note_channels.len();

    notes.sort_by(|a, b| {
        a.start_beat
            .total_cmp(&b.start_beat)
            .then(a.pitch.cmp(&b.pitch))
    });

    (track_name, notes, all_drums)
}

fn finish_note(
    held: &mut std::collections::HashMap<(u8, u8), (u64, u8)>,
    ch: u8,
    key: u8,
    end_tick: u64,
    tick_to_beat: f32,
    notes: &mut Vec<MidiNote>,
    note_channels: &mut Vec<u8>,
) {
    let Some((start_tick, vel)) = held.remove(&(ch, key)) else {
        return;
    };
    let start_beat = start_tick as f32 * tick_to_beat;
    let end_beat = end_tick as f32 * tick_to_beat;
    let length_beats = (end_beat - start_beat).max(0.05);
    notes.push(MidiNote {
        pitch: key,
        velocity: (vel as f32 / 127.0).clamp(0.0, 1.0),
        start_beat,
        length_beats,
    });
    note_channels.push(ch);
}

fn lossy_name(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}
