use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::{
    project::Project,
    render::{StereoFrame, loop_length_frames, mix_stereo_frame},
};

#[derive(Debug)]
struct PlaybackState {
    source: Option<Arc<Project>>,
    loop_frames: u64,
    /// Next stereo frame index to emit (monotonic; wraps modulo `loop_frames` when looping).
    position: u64,
    sample_rate: u32,
    playing: bool,
    looping: bool,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            source: None,
            loop_frames: 0,
            position: 0,
            sample_rate: crate::render::DEFAULT_SAMPLE_RATE,
            playing: false,
            looping: false,
        }
    }
}

pub struct AudioEngine {
    stream: Option<Stream>,
    state: Arc<Mutex<PlaybackState>>,
    last_error: Arc<Mutex<Option<String>>>,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self {
            stream: None,
            state: Arc::new(Mutex::new(PlaybackState::default())),
            last_error: Arc::new(Mutex::new(None)),
        }
    }
}

impl AudioEngine {
    pub fn play(&mut self, project: &Project, looping: bool) -> Result<()> {
        self.stop();

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no default output device is available"))?;
        let supported = device.default_output_config()?;
        let sample_rate = supported.sample_rate();
        let config = supported.config();

        let source = Arc::new(project.clone());
        let loop_frames = loop_length_frames(&source, sample_rate);

        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow!("audio state lock poisoned"))?;
            state.source = Some(source);
            state.loop_frames = loop_frames;
            state.position = 0;
            state.sample_rate = sample_rate;
            state.playing = true;
            state.looping = looping;
        }

        let state = Arc::clone(&self.state);
        let last_error = Arc::clone(&self.last_error);
        let channels = config.channels as usize;
        let err_fn = move |err: cpal::StreamError| {
            if let Ok(mut slot) = last_error.lock() {
                *slot = Some(err.to_string());
            }
        };

        let stream = match supported.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(&device, &config, channels, state, err_fn),
            SampleFormat::I16 => build_stream::<i16>(&device, &config, channels, state, err_fn),
            SampleFormat::U16 => build_stream::<u16>(&device, &config, channels, state, err_fn),
            other => bail!("unsupported output sample format: {other:?}"),
        }
        .context("failed to create output stream")?;
        stream.play().context("failed to start output stream")?;
        self.stream = Some(stream);
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stream = None;
        if let Ok(mut state) = self.state.lock() {
            state.playing = false;
            state.position = 0;
            state.source = None;
            state.loop_frames = 0;
        }
    }

    pub fn is_playing(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.playing)
            .unwrap_or(false)
    }

    pub fn playback_progress(&self) -> f32 {
        self.state
            .lock()
            .ok()
            .map(|state| {
                if state.loop_frames == 0 {
                    return 0.0;
                }
                if state.looping {
                    (state.position % state.loop_frames) as f32 / state.loop_frames as f32
                } else if !state.playing && state.position >= state.loop_frames {
                    1.0
                } else {
                    (state.position.min(state.loop_frames)) as f32 / state.loop_frames as f32
                }
            })
            .unwrap_or(0.0)
            .clamp(0.0, 1.0)
    }

    pub fn take_error(&self) -> Option<String> {
        self.last_error.lock().ok()?.take()
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    state: Arc<Mutex<PlaybackState>>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<Stream, cpal::BuildStreamError>
where
    T: SizedSample + Sample + FromSample<f32>,
{
    device.build_output_stream(
        config,
        move |output: &mut [T], _| write_output(output, channels, &state),
        err_fn,
        None,
    )
}

fn write_output<T>(output: &mut [T], channels: usize, state: &Arc<Mutex<PlaybackState>>)
where
    T: Sample + FromSample<f32>,
{
    let Ok(mut state) = state.lock() else {
        for sample in output {
            *sample = T::from_sample(0.0);
        }
        return;
    };

    for frame in output.chunks_mut(channels) {
        let stereo = if state.playing {
            match &state.source {
                None => StereoFrame::default(),
                Some(project) => {
                    if state.loop_frames == 0 {
                        state.playing = false;
                        StereoFrame::default()
                    } else if state.looping {
                        let idx = state.position % state.loop_frames;
                        let stereo = mix_stereo_frame(project, idx, state.sample_rate);
                        state.position += 1;
                        stereo
                    } else if state.position >= state.loop_frames {
                        state.playing = false;
                        StereoFrame::default()
                    } else {
                        let idx = state.position;
                        let stereo = mix_stereo_frame(project, idx, state.sample_rate);
                        state.position += 1;
                        stereo
                    }
                }
            }
        } else {
            StereoFrame::default()
        };

        for (index, sample) in frame.iter_mut().enumerate() {
            let value = match index {
                0 => stereo.left,
                1 => stereo.right,
                _ => (stereo.left + stereo.right) * 0.5,
            };
            *sample = T::from_sample(value);
        }
    }
}
