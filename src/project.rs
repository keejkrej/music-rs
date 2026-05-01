use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

pub const BEATS_PER_BAR: f32 = 4.0;
pub const MIN_LOOP_BARS: u32 = 4;
pub const MAX_LOOP_BARS: u32 = 16;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub name: String,
    pub tempo_bpm: f32,
    pub loop_start_bar: u32,
    pub loop_bars: u32,
    pub master_gain: f32,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub instrument: Instrument,
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub clips: Vec<Clip>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Instrument {
    Synth { waveform: Waveform },
    DrumSampler,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Waveform {
    Sine,
    Square,
    Saw,
    Triangle,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Clip {
    pub id: String,
    pub name: String,
    pub start_beat: f32,
    pub length_beats: f32,
    pub notes: Vec<MidiNote>,
}

/// One note as a JSON array: `[pitch, velocity_midi, start_beat, length_beats]`.
/// `velocity_midi` is a standard MIDI note-on velocity (0–127); 0 is treated as silence.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiNote {
    pub pitch: u8,
    pub velocity: f32,
    pub start_beat: f32,
    pub length_beats: f32,
}

fn velocity_unit_to_midi_byte(v: f32) -> u8 {
    if v <= 0.0 {
        0
    } else {
        ((v * 127.0).round() as i32).clamp(1, 127) as u8
    }
}

fn velocity_midi_byte_to_unit(vel: u8) -> f32 {
    if vel == 0 {
        0.0
    } else {
        (vel as f32 / 127.0).clamp(0.0, 1.0)
    }
}

impl Serialize for MidiNote {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let vel = velocity_unit_to_midi_byte(self.velocity);
        (
            self.pitch,
            vel,
            self.start_beat,
            self.length_beats,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MidiNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (pitch, vel_midi, start_beat, length_beats) =
            <(u8, u8, f32, f32)>::deserialize(deserializer)?;
        if vel_midi > 127 {
            return Err(Error::custom(
                "MIDI velocity must be between 0 and 127",
            ));
        }
        Ok(MidiNote {
            pitch,
            velocity: velocity_midi_byte_to_unit(vel_midi),
            start_beat,
            length_beats,
        })
    }
}

impl Default for Project {
    fn default() -> Self {
        Self::blank()
    }
}

impl Project {
    pub fn blank() -> Self {
        Self {
            name: "Untitled Project".to_owned(),
            tempo_bpm: 120.0,
            loop_start_bar: 0,
            loop_bars: 8,
            master_gain: 0.85,
            tracks: Vec::new(),
        }
    }

    pub fn loop_length_beats(&self) -> f32 {
        self.loop_bars as f32 * BEATS_PER_BAR
    }

    pub fn loop_start_beat(&self) -> f32 {
        self.loop_start_bar as f32 * BEATS_PER_BAR
    }

    pub fn clamp_settings(&mut self) {
        self.tempo_bpm = self.tempo_bpm.clamp(40.0, 240.0);
        self.loop_bars = self.loop_bars.clamp(MIN_LOOP_BARS, MAX_LOOP_BARS);
        self.master_gain = self.master_gain.clamp(0.0, 1.25);
        for track in &mut self.tracks {
            track.gain = track.gain.clamp(0.0, 1.5);
            track.pan = track.pan.clamp(-1.0, 1.0);
            for clip in &mut track.clips {
                clip.start_beat = clip.start_beat.max(0.0);
                clip.length_beats = clip
                    .length_beats
                    .clamp(0.25, MAX_LOOP_BARS as f32 * BEATS_PER_BAR);
                for note in &mut clip.notes {
                    note.velocity = note.velocity.clamp(0.0, 1.0);
                    note.length_beats = note.length_beats.clamp(0.05, clip.length_beats);
                }
            }
        }
    }

    pub fn create_track(&mut self, name: impl Into<String>, instrument: Instrument) -> String {
        self.tracks.push(Track {
            id: new_id(),
            name: name.into(),
            instrument,
            gain: 0.85,
            pan: 0.0,
            mute: false,
            solo: false,
            clips: Vec::new(),
        });
        self.tracks.last().map(|track| track.id.clone()).unwrap()
    }

    pub fn add_clip(
        &mut self,
        track_id: &str,
        name: impl Into<String>,
        start_beat: f32,
        length_beats: f32,
    ) -> Option<&str> {
        let track = self.track_mut(track_id)?;
        track.clips.push(Clip {
            id: new_id(),
            name: name.into(),
            start_beat,
            length_beats,
            notes: Vec::new(),
        });
        Some(track.clips.last().map(|clip| clip.id.as_str()).unwrap())
    }

    pub fn track(&self, id: &str) -> Option<&Track> {
        self.tracks.iter().find(|track| track.id == id)
    }

    pub fn track_mut(&mut self, id: &str) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|track| track.id == id)
    }

    pub fn first_track_id_for(&self, instrument: fn(&Instrument) -> bool) -> Option<String> {
        self.tracks
            .iter()
            .find(|track| instrument(&track.instrument))
            .map(|track| track.id.clone())
    }

    pub fn compact_summary(&self) -> String {
        let mut lines = vec![format!(
            "{}: {:.1} BPM, {} bars, {} tracks",
            self.name,
            self.tempo_bpm,
            self.loop_bars,
            self.tracks.len()
        )];
        for track in &self.tracks {
            let instrument = match &track.instrument {
                Instrument::Synth { waveform } => format!("{waveform:?} synth"),
                Instrument::DrumSampler => "drum sampler".to_owned(),
            };
            let note_count: usize = track.clips.iter().map(|clip| clip.notes.len()).sum();
            lines.push(format!(
                "- {} [{}]: {}, {} clips, {} notes, gain {:.2}, pan {:.2}",
                track.name,
                track.id,
                instrument,
                track.clips.len(),
                note_count,
                track.gain,
                track.pan
            ));
        }
        lines.join("\n")
    }

}

impl Clip {
    pub fn contains_beat(&self, beat: f32) -> bool {
        beat >= self.start_beat && beat < self.start_beat + self.length_beats
    }

    pub fn add_note(&mut self, note: MidiNote) {
        self.notes.push(note);
        self.notes.sort_by(|a, b| {
            a.start_beat
                .total_cmp(&b.start_beat)
                .then(a.pitch.cmp(&b.pitch))
        });
    }
}

pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn midi_note_name(pitch: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = pitch as i16 / 12 - 1;
    format!("{}{}", NAMES[pitch as usize % 12], octave)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_note_serializes_as_four_element_array() {
        let note = MidiNote {
            pitch: 60,
            velocity: 0.5,
            start_beat: 1.0,
            length_beats: 0.5,
        };
        let v = serde_json::to_value(&note).unwrap();
        assert!(v.is_array());
        let round: MidiNote = serde_json::from_value(v).unwrap();
        assert_eq!(round.pitch, note.pitch);
        assert!((round.velocity - note.velocity).abs() < 0.02);
        assert_eq!(round.start_beat, note.start_beat);
        assert_eq!(round.length_beats, note.length_beats);
    }

    #[test]
    fn default_project_is_blank() {
        let project = Project::default();
        assert_eq!(project.name, "Untitled Project");
        assert_eq!(project.loop_bars, 8);
        assert!(project.tracks.is_empty());
    }

    #[test]
    fn clip_keeps_notes_ordered() {
        let mut clip = Clip {
            id: new_id(),
            name: "x".to_owned(),
            start_beat: 0.0,
            length_beats: 4.0,
            notes: Vec::new(),
        };
        clip.add_note(MidiNote {
            pitch: 64,
            velocity: 0.5,
            start_beat: 2.0,
            length_beats: 1.0,
        });
        clip.add_note(MidiNote {
            pitch: 60,
            velocity: 0.5,
            start_beat: 0.0,
            length_beats: 1.0,
        });
        assert_eq!(clip.notes[0].pitch, 60);
    }
}
