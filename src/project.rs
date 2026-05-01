use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MidiNote {
    pub pitch: u8,
    pub velocity: f32,
    pub start_beat: f32,
    pub length_beats: f32,
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

    pub fn birthday_demo() -> Self {
        let mut project = Self {
            name: "Birthday Demo".to_owned(),
            tempo_bpm: 112.0,
            loop_start_bar: 0,
            loop_bars: 8,
            master_gain: 0.78,
            tracks: Vec::new(),
        };
        let loop_len = project.loop_length_beats();

        let melody = project.create_track(
            "Birthday Lead",
            Instrument::Synth {
                waveform: Waveform::Triangle,
            },
        );
        if let Some(clip_id) = project.add_clip(&melody, "Melody", 0.0, loop_len) {
            let clip_id = clip_id.to_owned();
            if let Some(track) = project.track_mut(&melody) {
                if let Some(clip) = track.clips.iter_mut().find(|clip| clip.id == clip_id) {
                    for note in birthday_melody() {
                        clip.add_note(note);
                    }
                }
            }
        }

        let chords = project.create_track(
            "Chords",
            Instrument::Synth {
                waveform: Waveform::Saw,
            },
        );
        if let Some(clip_id) = project.add_clip(&chords, "Block Chords", 0.0, loop_len) {
            let clip_id = clip_id.to_owned();
            if let Some(track) = project.track_mut(&chords) {
                track.gain = 0.32;
                track.pan = -0.18;
                if let Some(clip) = track.clips.iter_mut().find(|clip| clip.id == clip_id) {
                    for note in birthday_chords() {
                        clip.add_note(note);
                    }
                }
            }
        }

        let bass = project.create_track(
            "Bass",
            Instrument::Synth {
                waveform: Waveform::Square,
            },
        );
        if let Some(clip_id) = project.add_clip(&bass, "Root Notes", 0.0, loop_len) {
            let clip_id = clip_id.to_owned();
            if let Some(track) = project.track_mut(&bass) {
                track.gain = 0.46;
                track.pan = 0.12;
                if let Some(clip) = track.clips.iter_mut().find(|clip| clip.id == clip_id) {
                    for note in birthday_bassline() {
                        clip.add_note(note);
                    }
                }
            }
        }

        let drums = project.create_track("Drums", Instrument::DrumSampler);
        if let Some(clip_id) = project.add_clip(&drums, "Simple Beat", 0.0, loop_len) {
            let clip_id = clip_id.to_owned();
            if let Some(track) = project.track_mut(&drums) {
                track.gain = 0.55;
                if let Some(clip) = track.clips.iter_mut().find(|clip| clip.id == clip_id) {
                    for note in birthday_drums() {
                        clip.add_note(note);
                    }
                }
            }
        }

        project
    }

    pub fn teen_spirit_demo() -> Self {
        let mut project = Self {
            name: "Teen Spirit Intro Snippet".to_owned(),
            tempo_bpm: 116.0,
            loop_start_bar: 0,
            loop_bars: 4,
            master_gain: 0.8,
            tracks: Vec::new(),
        };
        let loop_len = project.loop_length_beats();

        let guitar = project.create_track(
            "Fuzzy Guitar",
            Instrument::Synth {
                waveform: Waveform::Saw,
            },
        );
        if let Some(clip_id) = project.add_clip(&guitar, "Power Chord Riff", 0.0, loop_len) {
            let clip_id = clip_id.to_owned();
            if let Some(track) = project.track_mut(&guitar) {
                track.gain = 0.48;
                track.pan = -0.12;
                if let Some(clip) = track.clips.iter_mut().find(|clip| clip.id == clip_id) {
                    for note in teen_spirit_power_chords() {
                        clip.add_note(note);
                    }
                }
            }
        }

        let bass = project.create_track(
            "Bass",
            Instrument::Synth {
                waveform: Waveform::Square,
            },
        );
        if let Some(clip_id) = project.add_clip(&bass, "Driving Roots", 0.0, loop_len) {
            let clip_id = clip_id.to_owned();
            if let Some(track) = project.track_mut(&bass) {
                track.gain = 0.52;
                track.pan = 0.08;
                if let Some(clip) = track.clips.iter_mut().find(|clip| clip.id == clip_id) {
                    for note in teen_spirit_bassline() {
                        clip.add_note(note);
                    }
                }
            }
        }

        let drums = project.create_track("Drums", Instrument::DrumSampler);
        if let Some(clip_id) = project.add_clip(&drums, "Grunge Beat", 0.0, loop_len) {
            let clip_id = clip_id.to_owned();
            if let Some(track) = project.track_mut(&drums) {
                track.gain = 0.66;
                if let Some(clip) = track.clips.iter_mut().find(|clip| clip.id == clip_id) {
                    for note in teen_spirit_drums() {
                        clip.add_note(note);
                    }
                }
            }
        }

        project
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

    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn from_json(json: &str) -> anyhow::Result<Self> {
        let mut project: Self = serde_json::from_str(json)?;
        project.clamp_settings();
        Ok(project)
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

fn birthday_melody() -> Vec<MidiNote> {
    [
        (67, 0.0, 0.45),
        (67, 0.5, 0.45),
        (69, 1.0, 0.95),
        (67, 2.0, 0.95),
        (72, 3.0, 0.95),
        (71, 4.0, 1.8),
        (67, 8.0, 0.45),
        (67, 8.5, 0.45),
        (69, 9.0, 0.95),
        (67, 10.0, 0.95),
        (74, 11.0, 0.95),
        (72, 12.0, 1.8),
        (67, 16.0, 0.45),
        (67, 16.5, 0.45),
        (79, 17.0, 0.95),
        (76, 18.0, 0.95),
        (72, 19.0, 0.95),
        (71, 20.0, 0.95),
        (69, 21.0, 1.8),
        (77, 24.0, 0.45),
        (77, 24.5, 0.45),
        (76, 25.0, 0.95),
        (72, 26.0, 0.95),
        (74, 27.0, 0.95),
        (72, 28.0, 2.6),
    ]
    .into_iter()
    .map(|(pitch, start_beat, length_beats)| MidiNote {
        pitch,
        velocity: 0.78,
        start_beat,
        length_beats,
    })
    .collect()
}

fn birthday_chords() -> Vec<MidiNote> {
    let bars = [
        [48, 52, 55],
        [48, 52, 55],
        [55, 59, 62],
        [48, 52, 55],
        [53, 57, 60],
        [48, 52, 55],
        [55, 59, 62],
        [48, 52, 55],
    ];
    bars.into_iter()
        .enumerate()
        .flat_map(|(bar, chord)| {
            chord.into_iter().map(move |pitch| MidiNote {
                pitch,
                velocity: 0.34,
                start_beat: bar as f32 * BEATS_PER_BAR,
                length_beats: 3.6,
            })
        })
        .collect()
}

fn birthday_bassline() -> Vec<MidiNote> {
    let roots = [36, 36, 43, 36, 41, 36, 43, 36];
    roots
        .into_iter()
        .enumerate()
        .flat_map(|(bar, pitch)| {
            [0.0, 2.0].into_iter().map(move |beat| MidiNote {
                pitch,
                velocity: 0.62,
                start_beat: bar as f32 * BEATS_PER_BAR + beat,
                length_beats: 1.45,
            })
        })
        .collect()
}

fn birthday_drums() -> Vec<MidiNote> {
    let mut notes = Vec::new();
    for bar in 0..8 {
        let base = bar as f32 * BEATS_PER_BAR;
        notes.push(MidiNote {
            pitch: 36,
            velocity: 0.7,
            start_beat: base,
            length_beats: 0.16,
        });
        notes.push(MidiNote {
            pitch: 38,
            velocity: 0.42,
            start_beat: base + 2.0,
            length_beats: 0.12,
        });
        for step in 0..8 {
            notes.push(MidiNote {
                pitch: 42,
                velocity: if step % 2 == 0 { 0.28 } else { 0.18 },
                start_beat: base + step as f32 * 0.5,
                length_beats: 0.06,
            });
        }
    }
    notes
}

fn teen_spirit_power_chords() -> Vec<MidiNote> {
    let changes = [
        (41, 0.0),
        (46, 2.0),
        (44, 4.0),
        (49, 6.0),
        (41, 8.0),
        (46, 10.0),
        (44, 12.0),
        (49, 14.0),
    ];
    changes
        .into_iter()
        .flat_map(|(root, start)| {
            [root, root + 7, root + 12]
                .into_iter()
                .flat_map(move |pitch| {
                    [
                        MidiNote {
                            pitch,
                            velocity: 0.82,
                            start_beat: start,
                            length_beats: 0.42,
                        },
                        MidiNote {
                            pitch,
                            velocity: 0.52,
                            start_beat: start + 0.75,
                            length_beats: 0.18,
                        },
                        MidiNote {
                            pitch,
                            velocity: 0.76,
                            start_beat: start + 1.0,
                            length_beats: 0.48,
                        },
                    ]
                })
        })
        .collect()
}

fn teen_spirit_bassline() -> Vec<MidiNote> {
    let changes = [
        (29, 0.0),
        (34, 2.0),
        (32, 4.0),
        (37, 6.0),
        (29, 8.0),
        (34, 10.0),
        (32, 12.0),
        (37, 14.0),
    ];
    changes
        .into_iter()
        .flat_map(|(pitch, start)| {
            [0.0, 0.75, 1.0].into_iter().map(move |offset| MidiNote {
                pitch,
                velocity: if offset == 0.75 { 0.5 } else { 0.7 },
                start_beat: start + offset,
                length_beats: if offset == 0.75 { 0.16 } else { 0.42 },
            })
        })
        .collect()
}

fn teen_spirit_drums() -> Vec<MidiNote> {
    let mut notes = Vec::new();
    for bar in 0..4 {
        let base = bar as f32 * BEATS_PER_BAR;
        for beat in [0.0, 2.0] {
            notes.push(MidiNote {
                pitch: 36,
                velocity: 0.9,
                start_beat: base + beat,
                length_beats: 0.18,
            });
        }
        for beat in [1.0, 3.0] {
            notes.push(MidiNote {
                pitch: 38,
                velocity: 0.82,
                start_beat: base + beat,
                length_beats: 0.16,
            });
        }
        for step in 0..8 {
            notes.push(MidiNote {
                pitch: 42,
                velocity: if step % 2 == 0 { 0.42 } else { 0.24 },
                start_beat: base + step as f32 * 0.5,
                length_beats: 0.07,
            });
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_round_trips_json() {
        let project = Project::default();
        let json = project.to_json().unwrap();
        let loaded = Project::from_json(&json).unwrap();
        assert_eq!(loaded.tempo_bpm, project.tempo_bpm);
        assert_eq!(loaded.tracks.len(), project.tracks.len());
    }

    #[test]
    fn default_project_is_blank() {
        let project = Project::default();
        assert_eq!(project.name, "Untitled Project");
        assert_eq!(project.loop_bars, 8);
        assert!(project.tracks.is_empty());
    }

    #[test]
    fn demo_projects_are_playable() {
        for project in [Project::birthday_demo(), Project::teen_spirit_demo()] {
            assert!(!project.tracks.is_empty());
            assert!(
                project
                    .tracks
                    .iter()
                    .flat_map(|track| &track.clips)
                    .any(|clip| !clip.notes.is_empty())
            );
        }
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
