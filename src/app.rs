use std::{
    path::{Path, PathBuf},
    time::Duration,
};

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

const SAMPLE_PROJECT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/projects");

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
        let project = Project::blank();
        let selected_track = project.tracks.first().map(|track| track.id.clone());
        let selected_clip = project
            .tracks
            .first()
            .and_then(|track| track.clips.first())
            .map(|clip| clip.id.clone());
        Self {
            project,
            undo: UndoStack::default(),
            audio: AudioEngine::default(),
            control_server,
            selected_track,
            selected_clip,
            status: "Ready".to_owned(),
            loop_playback: true,
            project_path: default_project_path().display().to_string(),
            export_path: default_audio_dir()
                .join("music-rs-loop.wav")
                .display()
                .to_string(),
            draft_pitch: 60,
            draft_start: 0.0,
            draft_len: 0.5,
            draft_velocity: 0.75,
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New Blank").clicked() {
                    self.new_blank_project();
                    ui.close();
                }
                if ui.button("Open Project...").clicked() {
                    self.open_project_dialog();
                    ui.close();
                }
                ui.separator();
                if ui.button("Save").clicked() {
                    self.run_io(|this| this.save_project());
                    ui.close();
                }
                if ui.button("Save As...").clicked() {
                    self.save_project_as_dialog();
                    ui.close();
                }
                ui.separator();
                if ui.button("Export WAV...").clicked() {
                    self.export_wav_dialog();
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            ui.separator();
            ui.label(&self.project.name);
            if let Some(file_name) = Path::new(&self.project_path)
                .file_name()
                .and_then(|name| name.to_str())
            {
                ui.label(format!("({file_name})"));
            }
        });
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
        let solo_active = self.project.tracks.iter().any(|track| track.solo);
        for (row, track) in self.project.tracks.iter().enumerate() {
            let y = rect.top() + 8.0 + row as f32 * (row_height + 6.0);
            if y + row_height > rect.bottom() {
                break;
            }
            let track_audible = !track.mute && (!solo_active || track.solo);
            let row_rect = Rect::from_min_size(
                Pos2::new(rect.left(), y),
                Vec2::new(rect.width(), row_height),
            );
            if !track_audible {
                painter.rect_filled(row_rect, 2.0, Color32::from_rgb(34, 34, 38));
            }
            let track_label = if track.mute {
                format!("{}  MUTE", track.name)
            } else if track.solo {
                format!("{}  SOLO", track.name)
            } else if solo_active {
                format!("{}  SILENT", track.name)
            } else {
                track.name.clone()
            };
            let track_label_color = if track.mute {
                Color32::from_rgb(210, 110, 105)
            } else if track.solo {
                Color32::from_rgb(255, 235, 130)
            } else if track_audible {
                Color32::WHITE
            } else {
                Color32::from_rgb(145, 145, 150)
            };
            painter.text(
                Pos2::new(rect.left() + 6.0, y + 4.0),
                egui::Align2::LEFT_TOP,
                track_label,
                egui::TextStyle::Small.resolve(ui.style()),
                track_label_color,
            );
            for clip in &track.clips {
                let x = rect.left() + clip.start_beat * beat_width;
                let w = clip.length_beats * beat_width;
                let clip_rect =
                    Rect::from_min_size(Pos2::new(x, y), Vec2::new(w.max(8.0), row_height));
                let selected = self.selected_clip.as_deref() == Some(clip.id.as_str());
                let clip_fill = if track.mute {
                    Color32::from_rgb(70, 54, 56)
                } else if !track_audible {
                    Color32::from_rgb(50, 55, 62)
                } else if selected {
                    Color32::from_rgb(82, 142, 176)
                } else {
                    Color32::from_rgb(68, 92, 118)
                };
                let clip_stroke = if track.mute {
                    Color32::from_rgb(165, 90, 92)
                } else if !track_audible {
                    Color32::from_rgb(90, 95, 104)
                } else {
                    Color32::from_rgb(145, 170, 190)
                };
                painter.rect_filled(clip_rect, 3.0, clip_fill);
                painter.rect_stroke(
                    clip_rect,
                    3.0,
                    Stroke::new(1.0, clip_stroke),
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

        ui.horizontal_wrapped(|ui| {
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

        let grid_width = ui.available_width().max(720.0);
        egui::ScrollArea::horizontal()
            .id_salt("piano_roll_scroll")
            .show(ui, |ui| {
                let desired = Vec2::new(grid_width, 280.0);
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

                let clip_playhead = self.current_playhead_beat() - clip.start_beat;
                if (0.0..=clip.length_beats).contains(&clip_playhead) {
                    let playhead_x = rect.left() + clip_playhead * beat_width;
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
            });
    }

    fn current_playhead_beat(&self) -> f32 {
        self.project.loop_start_beat()
            + self.audio.playback_progress() * self.project.loop_length_beats()
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

    fn new_blank_project(&mut self) {
        self.audio.stop();
        self.project = Project::blank();
        self.undo.clear();
        self.project_path = default_project_path().display().to_string();
        self.ensure_selection();
        self.status = "New blank project".to_owned();
    }

    fn open_project_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Music RS project", &["json"])
            .set_directory(default_open_dir())
            .pick_file()
        else {
            self.status = "Open canceled".to_owned();
            return;
        };
        self.run_io(|this| this.load_project_from_path(&path));
    }

    fn save_project_as_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Music RS project", &["json"])
            .set_directory(default_open_dir())
            .set_file_name(project_file_name(&self.project.name))
            .save_file()
        else {
            self.status = "Save canceled".to_owned();
            return;
        };
        self.project_path = path.display().to_string();
        self.run_io(|this| this.save_project());
    }

    fn export_wav_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .set_directory(default_audio_dir())
            .set_file_name(project_wav_name(&self.project.name))
            .save_file()
        else {
            self.status = "Export canceled".to_owned();
            return;
        };
        self.export_path = path.display().to_string();
        self.run_io(|this| {
            export_wav(&this.project, &this.export_path)
                .with_context(|| format!("exporting {}", this.export_path))
        });
    }

    fn save_project(&self) -> Result<()> {
        std::fs::write(&self.project_path, self.project.to_json()?)
            .with_context(|| format!("saving {}", self.project_path))
    }

    fn load_project(&mut self) -> Result<()> {
        let json = std::fs::read_to_string(&self.project_path)
            .with_context(|| format!("loading {}", self.project_path))?;
        self.audio.stop();
        self.project = Project::from_json(&json)?;
        self.undo.clear();
        self.ensure_selection();
        Ok(())
    }

    fn load_project_from_path(&mut self, path: &Path) -> Result<()> {
        self.project_path = path.display().to_string();
        self.load_project()
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
            self.menu_bar(ui);
            ui.separator();
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

fn default_project_path() -> PathBuf {
    dirs::document_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("music-rs-project.json")
}

fn default_open_dir() -> PathBuf {
    let sample_dir = PathBuf::from(SAMPLE_PROJECT_DIR);
    if sample_dir.exists() {
        sample_dir
    } else {
        dirs::document_dir().unwrap_or_else(|| PathBuf::from("."))
    }
}

fn default_audio_dir() -> PathBuf {
    dirs::audio_dir()
        .or_else(dirs::document_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn project_file_name(name: &str) -> String {
    format!("{}.json", file_stem(name))
}

fn project_wav_name(name: &str) -> String {
    format!("{}.wav", file_stem(name))
}

fn file_stem(name: &str) -> String {
    let stem = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if stem.is_empty() {
        "music-rs-project".to_owned()
    } else {
        stem
    }
}
