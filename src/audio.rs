use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::{
    project::Project,
    render::{StereoFrame, render_project},
};

#[derive(Debug, Default)]
struct PlaybackState {
    frames: Vec<StereoFrame>,
    cursor: usize,
    playing: bool,
    looping: bool,
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
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no default output device is available"))?;
        let supported = device.default_output_config()?;
        let sample_rate = supported.sample_rate();
        let config = supported.config();
        let frames = render_project(project, sample_rate);

        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow!("audio state lock poisoned"))?;
            state.frames = frames;
            state.cursor = 0;
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
        if let Ok(mut state) = self.state.lock() {
            state.playing = false;
            state.cursor = 0;
        }
        self.stream = None;
    }

    pub fn is_playing(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.playing)
            .unwrap_or(false)
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
        let stereo = if state.playing && !state.frames.is_empty() {
            if state.cursor >= state.frames.len() {
                if state.looping {
                    state.cursor = 0;
                } else {
                    state.playing = false;
                }
            }
            if state.playing {
                let stereo = state.frames[state.cursor];
                state.cursor += 1;
                stereo
            } else {
                StereoFrame::default()
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
