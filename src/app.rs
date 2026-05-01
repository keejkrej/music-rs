use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use serde_json::json;

use crate::{
    audio::AudioEngine,
    commands::{EditCommand, apply_commands},
    control::{ControlRequest, ControlServer, error, ok},
    project::{Clip, Instrument, MidiNote, Project, Waveform, midi_note_name},
    render::export_wav,
    undo::{UndoStack, apply_undoable},
};

pub struct DawApp {
    project: Project,
    undo: UndoStack,
    audio: AudioEngine,
    control_server: Option<ControlServer>,
    selected_track: Option<String>,
    selected_clip: Option<String>,
    status: String,
    loop_playback: bool,
    project_path: String,
    export_path: String,
    draft_pitch: u8,
    draft_start: f32,
    draft_len: f32,
    draft_velocity: f32,
}

impl DawApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, control_server: Option<ControlServer>) -> Self {
        let project = Project::default();
        let selected_track = project.tracks.first().map(|track| track.id.clone());
        let selected_clip = project
            .tracks
            .first()
            .and_then(|track| track.clips.first())
            .map(|clip| clip.id.clone());
        let base = dirs::audio_dir()
            .or_else(dirs::document_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            project,
            undo: UndoStack::default(),
            audio: AudioEngine::default(),
            control_server,
            selected_track,
            selected_clip,
            status: "Ready".to_owned(),
            loop_playback: true,
            project_path: base.join("music-rs-loop.json").display().to_string(),
            export_path: base.join("music-rs-loop.wav").display().to_string(),
            draft_pitch: 60,
            draft_start: 0.0,
            draft_len: 0.5,
            draft_velocity: 0.75,
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Play").clicked() {
                match self.audio.play(&self.project, self.loop_playback) {
                    Ok(()) => self.status = "Playing".to_owned(),
                    Err(err) => self.status = format!("Audio error: {err}"),
                }
            }
            if ui.button("Stop").clicked() {
                self.audio.stop();
                self.status = "Stopped".to_owned();
            }
            ui.checkbox(&mut self.loop_playback, "Loop");

            let mut bpm = self.project.tempo_bpm;
            ui.label("BPM");
            if ui
                .add(
                    egui::DragValue::new(&mut bpm)
                        .range(40.0..=240.0)
                        .speed(0.5),
                )
                .changed()
            {
                let _ = self.edit_project(|project| {
                    apply_commands(project, vec![EditCommand::SetTempo { bpm }])
                });
            }

            let mut bars = self.project.loop_bars;
            ui.label("Bars");
            if ui
                .add(egui::DragValue::new(&mut bars).range(4..=16).speed(1.0))
                .changed()
            {
                let _ = self.edit_project(|project| {
                    apply_commands(project, vec![EditCommand::ArrangeLoop { bars }])
                });
            }

            ui.separator();
            if ui
                .add_enabled(self.undo.can_undo(), egui::Button::new("Undo"))
                .clicked()
            {
                match self.undo.undo(&mut self.project) {
                    Ok(()) => self.status = "Undone".to_owned(),
                    Err(err) => self.status = err.to_string(),
                }
            }
            if ui
                .add_enabled(self.undo.can_redo(), egui::Button::new("Redo"))
                .clicked()
            {
                match self.undo.redo(&mut self.project) {
                    Ok(()) => self.status = "Redone".to_owned(),
                    Err(err) => self.status = err.to_string(),
                }
            }

            ui.separator();
            if ui.button("Save").clicked() {
                self.run_io(|this| this.save_project());
            }
            if ui.button("Load").clicked() {
                self.run_io(|this| this.load_project());
            }
            if ui.button("Export WAV").clicked() {
                self.run_io(|this| {
                    export_wav(&this.project, &this.export_path)
                        .with_context(|| format!("exporting {}", this.export_path))
                });
            }
            if let Some(server) = &self.control_server {
                ui.label(format!("Control ws://{}", server.addr));
            }
            ui.label(&self.status);
        });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Tracks");
        if ui.button("Add Synth").clicked() {
            let _ = self.edit_project(|project| {
                project.create_track(
                    "Synth",
                    Instrument::Synth {
                        waveform: Waveform::Saw,
                    },
                );
                Ok(())
            });
            self.select_last_track();
        }
        if ui.button("Add Drums").clicked() {
            let _ = self.edit_project(|project| {
                project.create_track("Drums", Instrument::DrumSampler);
                Ok(())
            });
            self.select_last_track();
        }
        ui.separator();

        let track_summaries: Vec<_> = self
            .project
            .tracks
            .iter()
            .map(|track| {
                (
                    track.id.clone(),
                    track.name.clone(),
                    track.clips.first().map(|clip| clip.id.clone()),
                    instrument_label(&track.instrument).to_owned(),
                )
            })
            .collect();

        for (track_id, name, first_clip, instrument) in track_summaries {
            let selected = self.selected_track.as_deref() == Some(track_id.as_str());
            if ui
                .selectable_label(selected, format!("{name}\n{instrument}"))
                .clicked()
            {
                self.selected_track = Some(track_id);
                self.selected_clip = first_clip;
            }
        }
    }

    fn right_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Mixer");
        let track_ids: Vec<String> = self
            .project
            .tracks
            .iter()
            .map(|track| track.id.clone())
            .collect();
        for track_id in track_ids {
            let Some(track) = self.project.track(&track_id).cloned() else {
                continue;
            };
            ui.separator();
            ui.label(track.name);

            let mut gain = track.gain;
            if ui
                .add(egui::Slider::new(&mut gain, 0.0..=1.5).text("Gain"))
                .changed()
            {
                let _ = self.edit_project(|project| {
                    apply_commands(
                        project,
                        vec![EditCommand::SetMixer {
                            track_id: track_id.clone(),
                            gain: Some(gain),
                            pan: None,
                            mute: None,
                            solo: None,
                        }],
                    )
                });
            }

            let mut pan = track.pan;
            if ui
                .add(egui::Slider::new(&mut pan, -1.0..=1.0).text("Pan"))
                .changed()
            {
                let _ = self.edit_project(|project| {
                    apply_commands(
                        project,
                        vec![EditCommand::SetMixer {
                            track_id: track_id.clone(),
                            gain: None,
                            pan: Some(pan),
                            mute: None,
                            solo: None,
                        }],
                    )
                });
            }

            ui.horizontal(|ui| {
                let mut mute = track.mute;
                if ui.checkbox(&mut mute, "Mute").changed() {
                    let _ = self.edit_project(|project| {
                        apply_commands(
                            project,
                            vec![EditCommand::SetMixer {
                                track_id: track_id.clone(),
                                gain: None,
                                pan: None,
                                mute: Some(mute),
                                solo: None,
                            }],
                        )
                    });
                }
                let mut solo = track.solo;
                if ui.checkbox(&mut solo, "Solo").changed() {
                    let _ = self.edit_project(|project| {
                        apply_commands(
                            project,
                            vec![EditCommand::SetMixer {
                                track_id: track_id.clone(),
                                gain: None,
                                pan: None,
                                mute: None,
                                solo: Some(solo),
                            }],
                        )
                    });
                }
            });
        }
    }

    fn central_panel(&mut self, ui: &mut egui::Ui) {
        self.timeline(ui);
        ui.separator();
        self.piano_roll(ui);
        ui.separator();
        self.file_paths(ui);
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        ui.heading("Timeline");
        let desired = Vec2::new(ui.available_width(), 150.0);
        let (rect, _) = ui.allocate_exact_size(desired, Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, Color32::from_rgb(28, 30, 34));
        let beats = self.project.loop_length_beats();
        let beat_width = rect.width() / beats.max(1.0);

        for beat in 0..=beats as usize {
            let x = rect.left() + beat as f32 * beat_width;
            let color = if beat % 4 == 0 {
                Color32::from_rgb(110, 118, 128)
            } else {
                Color32::from_rgb(55, 60, 68)
            };
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(1.0, color),
            );
        }

        let row_height = 24.0;
        for (row, track) in self.project.tracks.iter().enumerate() {
            let y = rect.top() + 8.0 + row as f32 * (row_height + 6.0);
            if y + row_height > rect.bottom() {
                break;
            }
            painter.text(
                Pos2::new(rect.left() + 6.0, y + 4.0),
                egui::Align2::LEFT_TOP,
                &track.name,
                egui::TextStyle::Small.resolve(ui.style()),
                Color32::WHITE,
            );
            for clip in &track.clips {
                let x = rect.left() + clip.start_beat * beat_width;
                let w = clip.length_beats * beat_width;
                let clip_rect =
                    Rect::from_min_size(Pos2::new(x, y), Vec2::new(w.max(8.0), row_height));
                let selected = self.selected_clip.as_deref() == Some(clip.id.as_str());
                painter.rect_filled(
                    clip_rect,
                    3.0,
                    if selected {
                        Color32::from_rgb(82, 142, 176)
                    } else {
                        Color32::from_rgb(68, 92, 118)
                    },
                );
                painter.rect_stroke(
                    clip_rect,
                    3.0,
                    Stroke::new(1.0, Color32::from_rgb(145, 170, 190)),
                    StrokeKind::Outside,
                );
            }
        }

        let progress = self.audio.playback_progress();
        let playhead_x = rect.left() + rect.width() * progress;
        painter.line_segment(
            [
                Pos2::new(playhead_x, rect.top()),
                Pos2::new(playhead_x, rect.bottom()),
            ],
            Stroke::new(2.0, Color32::from_rgb(255, 235, 130)),
        );
        painter.circle_filled(
            Pos2::new(playhead_x, rect.top() + 5.0),
            4.0,
            Color32::from_rgb(255, 235, 130),
        );
    }

    fn piano_roll(&mut self, ui: &mut egui::Ui) {
        ui.heading("Piano Roll");
        let Some((track_id, clip_id, clip)) = self.selected_clip_data() else {
            ui.label("Select a clip");
            return;
        };

        ui.horizontal(|ui| {
            ui.label(format!("{} notes", clip.notes.len()));
            ui.label(format!("Pitch {}", midi_note_name(self.draft_pitch)));
            ui.add(egui::DragValue::new(&mut self.draft_pitch).range(0..=127));
            ui.label("Start");
            ui.add(
                egui::DragValue::new(&mut self.draft_start)
                    .range(0.0..=clip.length_beats)
                    .speed(0.25),
            );
            ui.label("Length");
            ui.add(
                egui::DragValue::new(&mut self.draft_len)
                    .range(0.05..=4.0)
                    .speed(0.1),
            );
            ui.label("Velocity");
            ui.add(
                egui::DragValue::new(&mut self.draft_velocity)
                    .range(0.0..=1.0)
                    .speed(0.05),
            );
            if ui.button("Add Note").clicked() {
                let note = MidiNote {
                    pitch: self.draft_pitch,
                    velocity: self.draft_velocity,
                    start_beat: self.draft_start,
                    length_beats: self.draft_len,
                };
                let _ = self.edit_project(|project| {
                    apply_commands(
                        project,
                        vec![EditCommand::AddNotes {
                            track_id: track_id.clone(),
                            clip_id: Some(clip_id.clone()),
                            notes: vec![note],
                        }],
                    )
                });
            }
            if ui.button("Delete Last").clicked() {
                let _ = self.edit_project(|project| {
                    let track = project
                        .track_mut(&track_id)
                        .context("selected track no longer exists")?;
                    let clip = track
                        .clips
                        .iter_mut()
                        .find(|clip| clip.id == clip_id)
                        .context("selected clip no longer exists")?;
                    clip.notes.pop();
                    Ok(())
                });
            }
        });

        let desired = Vec2::new(ui.available_width(), 280.0);
        let (rect, _) = ui.allocate_exact_size(desired, Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, Color32::from_rgb(20, 22, 25));
        let pitch_min = 36_u8;
        let pitch_max = 84_u8;
        let pitch_span = (pitch_max - pitch_min) as f32;
        let beat_width = rect.width() / clip.length_beats.max(1.0);
        let row_height = rect.height() / pitch_span;

        for beat in 0..=clip.length_beats.ceil() as usize {
            let x = rect.left() + beat as f32 * beat_width;
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(1.0, Color32::from_rgb(48, 52, 59)),
            );
        }
        for row in 0..=pitch_span as usize {
            let y = rect.top() + row as f32 * row_height;
            painter.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                Stroke::new(1.0, Color32::from_rgb(35, 38, 43)),
            );
        }

        for note in &clip.notes {
            if note.pitch < pitch_min || note.pitch > pitch_max {
                continue;
            }
            let x = rect.left() + note.start_beat * beat_width;
            let y = rect.bottom() - (note.pitch - pitch_min) as f32 * row_height;
            let note_rect = Rect::from_min_size(
                Pos2::new(x, y - row_height),
                Vec2::new(
                    (note.length_beats * beat_width).max(4.0),
                    row_height.max(4.0),
                ),
            );
            painter.rect_filled(note_rect, 2.0, Color32::from_rgb(210, 132, 74));
        }
    }

    fn file_paths(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Project");
            ui.text_edit_singleline(&mut self.project_path);
        });
        ui.horizontal(|ui| {
            ui.label("WAV");
            ui.text_edit_singleline(&mut self.export_path);
        });
    }

    fn selected_clip_data(&self) -> Option<(String, String, Clip)> {
        let track_id = self.selected_track.clone()?;
        let clip_id = self.selected_clip.clone()?;
        let track = self.project.track(&track_id)?;
        let clip = track.clips.iter().find(|clip| clip.id == clip_id)?.clone();
        Some((track_id, clip_id, clip))
    }

    fn edit_project<F>(&mut self, edit: F) -> Result<()>
    where
        F: FnOnce(&mut Project) -> Result<()>,
    {
        apply_undoable(&mut self.project, &mut self.undo, edit)?;
        self.ensure_selection();
        Ok(())
    }

    fn ensure_selection(&mut self) {
        let selected_track_exists = self
            .selected_track
            .as_deref()
            .and_then(|id| self.project.track(id))
            .is_some();
        if !selected_track_exists {
            self.selected_track = self.project.tracks.first().map(|track| track.id.clone());
        }
        let selected_clip_exists = self
            .selected_track
            .as_deref()
            .and_then(|track_id| self.project.track(track_id))
            .and_then(|track| {
                self.selected_clip
                    .as_deref()
                    .and_then(|clip_id| track.clips.iter().find(|clip| clip.id == clip_id))
            })
            .is_some();
        if !selected_clip_exists {
            self.selected_clip = self
                .selected_track
                .as_deref()
                .and_then(|track_id| self.project.track(track_id))
                .and_then(|track| track.clips.first())
                .map(|clip| clip.id.clone());
        }
    }

    fn select_last_track(&mut self) {
        self.selected_track = self.project.tracks.last().map(|track| track.id.clone());
        self.selected_clip = self
            .project
            .tracks
            .last()
            .and_then(|track| track.clips.first())
            .map(|clip| clip.id.clone());
    }

    fn run_io<F>(&mut self, f: F)
    where
        F: FnOnce(&mut Self) -> Result<()>,
    {
        match f(self) {
            Ok(()) => self.status = "Done".to_owned(),
            Err(err) => self.status = err.to_string(),
        }
    }

    fn save_project(&self) -> Result<()> {
        std::fs::write(&self.project_path, self.project.to_json()?)
            .with_context(|| format!("saving {}", self.project_path))
    }

    fn load_project(&mut self) -> Result<()> {
        let json = std::fs::read_to_string(&self.project_path)
            .with_context(|| format!("loading {}", self.project_path))?;
        self.project = Project::from_json(&json)?;
        self.undo.clear();
        self.ensure_selection();
        Ok(())
    }

    fn process_control_requests(&mut self) {
        let Some(server) = &self.control_server else {
            return;
        };

        let mut pending = Vec::new();
        while let Some(request) = server.try_recv() {
            pending.push(request);
            if pending.len() >= 64 {
                break;
            }
        }

        for pending in pending {
            let id = pending.envelope.id.clone();
            let reply = match self.handle_control_request(pending.envelope.request) {
                Ok(result) => ok(id, result),
                Err(err) => error(id, err),
            };
            if let Some(reply_tx) = pending.reply {
                let _ = reply_tx.send(reply);
            }
        }
    }

    fn handle_control_request(&mut self, request: ControlRequest) -> Result<serde_json::Value> {
        match request {
            ControlRequest::GetSummary => Ok(json!({
                "summary": self.project.compact_summary(),
                "tempo_bpm": self.project.tempo_bpm,
                "loop_bars": self.project.loop_bars,
                "tracks": self.project.tracks.iter().map(|track| {
                    json!({
                        "id": track.id,
                        "name": track.name,
                        "instrument": instrument_label(&track.instrument),
                        "clips": track.clips.iter().map(|clip| {
                            json!({
                                "id": clip.id,
                                "name": clip.name,
                                "start_beat": clip.start_beat,
                                "length_beats": clip.length_beats,
                                "note_count": clip.notes.len()
                            })
                        }).collect::<Vec<_>>()
                    })
                }).collect::<Vec<_>>()
            })),
            ControlRequest::GetProject => Ok(serde_json::to_value(&self.project)?),
            ControlRequest::ApplyCommands { commands } => {
                self.edit_project(|project| apply_commands(project, commands))?;
                self.status = "Controlled edit applied".to_owned();
                Ok(json!({"summary": self.project.compact_summary()}))
            }
            ControlRequest::Play { looping } => {
                let looping = looping.unwrap_or(self.loop_playback);
                self.audio.play(&self.project, looping)?;
                self.loop_playback = looping;
                self.status = "Playing".to_owned();
                Ok(json!({"playing": true, "looping": looping}))
            }
            ControlRequest::Stop => {
                self.audio.stop();
                self.status = "Stopped".to_owned();
                Ok(json!({"playing": false}))
            }
            ControlRequest::Undo => {
                self.undo.undo(&mut self.project)?;
                self.ensure_selection();
                self.status = "Undone".to_owned();
                Ok(json!({"summary": self.project.compact_summary()}))
            }
            ControlRequest::Redo => {
                self.undo.redo(&mut self.project)?;
                self.ensure_selection();
                self.status = "Redone".to_owned();
                Ok(json!({"summary": self.project.compact_summary()}))
            }
            ControlRequest::Save { path } => {
                if let Some(path) = path {
                    self.project_path = path;
                }
                self.save_project()?;
                self.status = "Saved".to_owned();
                Ok(json!({"path": self.project_path}))
            }
            ControlRequest::Load { path } => {
                self.project_path = path;
                self.load_project()?;
                self.status = "Loaded".to_owned();
                Ok(json!({"summary": self.project.compact_summary()}))
            }
            ControlRequest::ExportWav { path } => {
                self.export_path = path;
                export_wav(&self.project, &self.export_path)
                    .with_context(|| format!("exporting {}", self.export_path))?;
                self.status = "Exported WAV".to_owned();
                Ok(json!({"path": self.export_path}))
            }
        }
    }
}

impl eframe::App for DawApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.process_control_requests();
        if self.control_server.is_some() || self.audio.is_playing() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        if let Some(err) = self.audio.take_error() {
            self.status = format!("Audio error: {err}");
        }
        egui::Panel::top("transport").show_inside(ui, |ui| {
            self.top_bar(ui);
        });
        egui::Panel::left("tracks")
            .resizable(true)
            .default_size(170.0)
            .show_inside(ui, |ui| self.left_panel(ui));
        egui::Panel::right("mixer")
            .resizable(true)
            .default_size(210.0)
            .show_inside(ui, |ui| self.right_panel(ui));
        egui::CentralPanel::default().show_inside(ui, |ui| self.central_panel(ui));
    }
}

fn instrument_label(instrument: &Instrument) -> &'static str {
    match instrument {
        Instrument::Synth { .. } => "Synth",
        Instrument::DrumSampler => "Drums",
    }
}
