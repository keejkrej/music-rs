use std::{f32::consts::PI, path::Path};

use anyhow::Result;

use crate::project::{Instrument, Project, Track, Waveform};

pub const DEFAULT_SAMPLE_RATE: u32 = 44_100;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StereoFrame {
    pub left: f32,
    pub right: f32,
}

pub fn render_project(project: &Project, sample_rate: u32) -> Vec<StereoFrame> {
    let seconds_per_beat = 60.0 / project.tempo_bpm.max(1.0);
    let length_seconds = project.loop_length_beats() * seconds_per_beat;
    let frame_count = (length_seconds * sample_rate as f32).round() as usize;
    let solo_active = project.tracks.iter().any(|track| track.solo);
    let mut frames = vec![StereoFrame::default(); frame_count];

    for (sample_index, frame) in frames.iter_mut().enumerate() {
        let time_seconds = sample_index as f32 / sample_rate as f32;
        let beat = project.loop_start_beat() + time_seconds / seconds_per_beat;
        let mut left = 0.0;
        let mut right = 0.0;

        for track in &project.tracks {
            if !track_should_sound(track, solo_active) {
                continue;
            }
            let sample = render_track(track, beat, seconds_per_beat, sample_rate);
            let (track_left, track_right) = pan_stereo(sample * track.gain, track.pan);
            left += track_left;
            right += track_right;
        }

        frame.left = soft_limit(left * project.master_gain);
        frame.right = soft_limit(right * project.master_gain);
    }

    frames
}

pub fn export_wav(project: &Project, path: impl AsRef<Path>) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: DEFAULT_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for frame in render_project(project, DEFAULT_SAMPLE_RATE) {
        writer.write_sample(float_to_i16(frame.left))?;
        writer.write_sample(float_to_i16(frame.right))?;
    }
    writer.finalize()?;
    Ok(())
}

fn render_track(track: &Track, beat: f32, seconds_per_beat: f32, sample_rate: u32) -> f32 {
    let mut out = 0.0;
    for clip in &track.clips {
        if !clip.contains_beat(beat) {
            continue;
        }
        let clip_beat = beat - clip.start_beat;
        for note in &clip.notes {
            let note_end = note.start_beat + note.length_beats;
            if clip_beat < note.start_beat || clip_beat >= note_end {
                continue;
            }
            let note_time = (clip_beat - note.start_beat) * seconds_per_beat;
            let note_len = note.length_beats * seconds_per_beat;
            let amp = note.velocity * envelope(note_time, note_len);
            out += match track.instrument {
                Instrument::Synth { waveform } => {
                    synth_sample(note.pitch, note_time, waveform) * amp
                }
                Instrument::DrumSampler => drum_sample(note.pitch, note_time, sample_rate) * amp,
            };
        }
    }
    out
}

fn track_should_sound(track: &Track, solo_active: bool) -> bool {
    !track.mute && (!solo_active || track.solo)
}

fn synth_sample(pitch: u8, time: f32, waveform: Waveform) -> f32 {
    let freq = 440.0 * 2.0_f32.powf((pitch as f32 - 69.0) / 12.0);
    let phase = (time * freq).fract();
    match waveform {
        Waveform::Sine => (2.0 * PI * phase).sin(),
        Waveform::Square => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        Waveform::Saw => 2.0 * phase - 1.0,
        Waveform::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
    }
}

fn drum_sample(pitch: u8, time: f32, sample_rate: u32) -> f32 {
    match pitch {
        36 => {
            let sweep = 62.0 + 85.0 * (-time * 26.0).exp();
            (2.0 * PI * sweep * time).sin() * (-time * 10.0).exp()
        }
        38 => {
            let tone = (2.0 * PI * 185.0 * time).sin() * (-time * 18.0).exp();
            let noise = noise_at(time, sample_rate) * (-time * 22.0).exp();
            tone * 0.45 + noise * 0.75
        }
        42 | 44 | 46 => {
            let noise = noise_at(time, sample_rate);
            let metallic = (2.0 * PI * 7_200.0 * time).sin() * 0.25;
            (noise * 0.8 + metallic) * (-time * 55.0).exp()
        }
        _ => 0.0,
    }
}

fn envelope(time: f32, len: f32) -> f32 {
    let attack = 0.006;
    let release = 0.035_f32.min(len * 0.4);
    let attack_amp = (time / attack).clamp(0.0, 1.0);
    let release_amp = ((len - time) / release).clamp(0.0, 1.0);
    attack_amp.min(release_amp)
}

fn noise_at(time: f32, sample_rate: u32) -> f32 {
    let n = (time * sample_rate as f32) as u32;
    let mut x = n.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    x = ((x >> ((x >> 28) + 4)) ^ x).wrapping_mul(277_803_737);
    let value = ((x >> 22) ^ x) as f32 / u32::MAX as f32;
    value * 2.0 - 1.0
}

fn pan_stereo(sample: f32, pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    let left = sample * (1.0 - pan).min(1.0);
    let right = sample * (1.0 + pan).min(1.0);
    (left, right)
}

fn soft_limit(sample: f32) -> f32 {
    sample.tanh().clamp(-1.0, 1.0)
}

fn float_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        commands::{DrumStyle, EditCommand, apply_command},
        project::Project,
    };

    #[test]
    fn offline_render_has_expected_length() {
        let mut project = Project::default();
        project.tempo_bpm = 120.0;
        project.loop_bars = 4;
        let frames = render_project(&project, 1_000);
        assert_eq!(frames.len(), 8_000);
    }

    #[test]
    fn default_project_renders_audio() {
        let project = Project::default();
        let frames = render_project(&project, 8_000);
        let peak = frames
            .iter()
            .map(|frame| frame.left.abs().max(frame.right.abs()))
            .fold(0.0, f32::max);
        assert!(peak > 0.01);
    }

    #[test]
    fn render_stays_in_sample_bounds() {
        let mut project = Project::default();
        let drum_id = project.tracks[0].id.clone();
        apply_command(
            &mut project,
            EditCommand::MakeDrumPattern {
                track_id: Some(drum_id),
                bars: 4,
                style: DrumStyle::House,
            },
        )
        .unwrap();
        let frames = render_project(&project, 8_000);
        assert!(frames.iter().all(|frame| frame.left >= -1.0
            && frame.left <= 1.0
            && frame.right >= -1.0
            && frame.right <= 1.0));
    }

    #[test]
    fn wav_export_writes_stereo_file() {
        let project = Project::default();
        let path =
            std::env::temp_dir().join(format!("music-rs-test-{}.wav", crate::project::new_id()));
        export_wav(&project, &path).unwrap();
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, DEFAULT_SAMPLE_RATE);
        let _ = std::fs::remove_file(path);
    }
}
