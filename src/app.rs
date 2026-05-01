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
    project::{Clip, Instrument, MidiNote, Project, Waveform, BEATS_PER_BAR, midi_note_name},
    project_io,
    render::export_wav,
    undo::{UndoStack, apply_undoable},
};

const SAMPLE_PROJECT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/projects");
/// Timeline / piano-roll horizontal resolution (pixels per beat); scroll when the loop is wider.
const TIMELINE_PX_PER_BEAT: f32 = 28.0;
const TIMELINE_RULER_H: f32 = 30.0;
const TIMELINE_ROW_H: f32 = 22.0;
const TIMELINE_ROW_GAP: f32 = 5.0;
const PIANO_KEYS_W: f32 = 44.0;
const PIANO_RULER_H: f32 = 24.0;

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
    /// Edit / playback cursor (absolute beat). While playing, advanced from audio each frame.
    transport_playhead_beat: f32,
}

impl DawApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, control_server: Option<ControlServer>) -> Self {
        let project = Project::blank();
        let transport_playhead_beat = project.loop_start_beat();
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
            transport_playhead_beat,
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New").clicked() {
                    self.new_project();
                    ui.close();
                }
                if ui.button("Open").clicked() {
                    self.open_project_dialog();
                    ui.close();
                }
                ui.separator();
                if ui.button("Save").clicked() {
                    self.save_project_dialog();
                    ui.close();
                }
                ui.separator();
                if ui.button("Export").clicked() {
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
        });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Play").clicked() {
                match self.audio.play_from_beat(
                    &self.project,
                    self.loop_playback,
                    self.transport_playhead_beat,
                ) {
                    Ok(()) => self.status = "Playing".to_owned(),
                    Err(err) => self.status = format!("Audio error: {err}"),
                }
            }
            if ui.button("Stop").clicked() {
                if self.audio.is_playing() {
                    self.transport_playhead_beat = self.playhead_beat_from_audio();
                }
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
        self.sync_transport_from_audio();
        let width = ui.available_width();
        let total_h = ui.available_height();
        let sep = 4.0_f32;
        let top_h = ((total_h - sep) * 0.5).max(48.0);
        let bottom_h = (total_h - sep - top_h).max(48.0);

        ui.vertical(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(width, top_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.timeline(ui, top_h),
            );
            ui.add_space(sep);
            ui.allocate_ui_with_layout(
                Vec2::new(width, bottom_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.piano_roll(ui, bottom_h),
            );
        });
    }

    fn sync_transport_from_audio(&mut self) {
        if self.audio.is_playing() {
            self.transport_playhead_beat = self.playhead_beat_from_audio();
        }
    }

    fn playhead_beat_from_audio(&self) -> f32 {
        self.project.loop_start_beat()
            + self.audio.playback_progress() * self.project.loop_length_beats()
    }

    fn playhead_beat_display(&self) -> f32 {
        if self.audio.is_playing() {
            self.playhead_beat_from_audio()
        } else {
            self.transport_playhead_beat
        }
    }

    fn set_playhead_from_timeline(&mut self, content_rect: Rect, local_x: f32) {
        let loop_start = self.project.loop_start_beat();
        let loop_len = self.project.loop_length_beats().max(1.0);
        let frac = (local_x / content_rect.width()).clamp(0.0, 1.0);
        let beat = loop_start + frac * loop_len;
        self.transport_playhead_beat = beat;
        if self.audio.is_playing() {
            self.audio.seek_to_beat(&self.project, beat);
        }
    }

    fn timeline(&mut self, ui: &mut egui::Ui, panel_h: f32) {
        ui.set_min_height(panel_h);
        ui.set_max_height(panel_h);
        ui.vertical(|ui| {
            ui.heading("Timeline");
            let inner_h = ui.available_height().max(24.0);
            let loop_start = self.project.loop_start_beat();
            let loop_len = self.project.loop_length_beats().max(1.0);
            let viewport_w = ui.available_width();
            let content_width = (loop_len * TIMELINE_PX_PER_BEAT).max(viewport_w);
            let track_rows = self.project.tracks.len().max(1) as f32;
            let tracks_h = 8.0 + track_rows * (TIMELINE_ROW_H + TIMELINE_ROW_GAP);
            let content_h = TIMELINE_RULER_H + tracks_h;

            egui::ScrollArea::both()
                .id_salt("timeline_scroll")
                .max_width(viewport_w)
                .max_height(inner_h)
                .show(ui, |ui| {
                let desired = Vec2::new(content_width, content_h);
                let (outer_rect, outer_response) =
                    ui.allocate_exact_size(desired, Sense::click_and_drag());
                let painter = ui.painter_at(outer_rect);
                painter.rect_filled(outer_rect, 4.0, Color32::from_rgb(22, 24, 28));

                let ruler_rect = Rect::from_min_size(
                    outer_rect.min,
                    Vec2::new(outer_rect.width(), TIMELINE_RULER_H),
                );
                let tracks_rect = Rect::from_min_size(
                    Pos2::new(outer_rect.left(), ruler_rect.bottom()),
                    Vec2::new(outer_rect.width(), outer_rect.height() - TIMELINE_RULER_H),
                );
                let beat_w = outer_rect.width() / loop_len;

                // Ruler background
                painter.rect_filled(ruler_rect, 0.0, Color32::from_rgb(32, 34, 40));
                painter.line_segment(
                    [
                        Pos2::new(outer_rect.left(), ruler_rect.bottom()),
                        Pos2::new(outer_rect.right(), ruler_rect.bottom()),
                    ],
                    Stroke::new(1.0, Color32::from_rgb(60, 64, 72)),
                );

                // Sixteenth grid (ruler + tracks)
                for sixteenth in 0..=(loop_len * 4.0).ceil() as i32 {
                    let t = sixteenth as f32 / 4.0;
                    if t > loop_len + 0.001 {
                        break;
                    }
                    let x = outer_rect.left() + t * beat_w;
                    let abs_beat = loop_start + t;
                    let frac_bar = (abs_beat / BEATS_PER_BAR).fract().abs();
                    let is_bar_line = frac_bar < 0.02 || frac_bar > 0.98;
                    let is_beat = (sixteenth % 4) == 0;
                    let (w, c) = if is_bar_line {
                        (1.5, Color32::from_rgb(120, 126, 138))
                    } else if is_beat {
                        (1.0, Color32::from_rgb(75, 80, 90))
                    } else {
                        (1.0, Color32::from_rgb(45, 48, 55))
                    };
                    painter.line_segment(
                        [
                            Pos2::new(x, ruler_rect.top()),
                            Pos2::new(x, outer_rect.bottom()),
                        ],
                        Stroke::new(w, c),
                    );
                }

                // Bar numbers on ruler (1-based, absolute bar index)
                let mut bar_beat = loop_start;
                while bar_beat < loop_start + loop_len + 0.001 {
                    let x = outer_rect.left() + (bar_beat - loop_start) * beat_w;
                    let bar_1based = ((bar_beat / BEATS_PER_BAR).floor() as i32).saturating_add(1);
                    painter.text(
                        Pos2::new(x + 3.0, ruler_rect.top() + 4.0),
                        egui::Align2::LEFT_TOP,
                        format!("{bar_1based}"),
                        egui::TextStyle::Small.resolve(ui.style()),
                        Color32::from_rgb(200, 204, 212),
                    );
                    bar_beat += BEATS_PER_BAR;
                }

                painter.text(
                    Pos2::new(outer_rect.right() - 4.0, ruler_rect.top() + 4.0),
                    egui::Align2::RIGHT_TOP,
                    format!("{:.0} BPM", self.project.tempo_bpm),
                    egui::TextStyle::Small.resolve(ui.style()),
                    Color32::from_rgb(130, 135, 145),
                );

                let solo_active = self.project.tracks.iter().any(|track| track.solo);
                for (row, track) in self.project.tracks.iter().enumerate() {
                    let y = tracks_rect.top()
                        + 6.0
                        + row as f32 * (TIMELINE_ROW_H + TIMELINE_ROW_GAP);
                    let track_audible = !track.mute && (!solo_active || track.solo);
                    let row_rect = Rect::from_min_size(
                        Pos2::new(tracks_rect.left(), y),
                        Vec2::new(tracks_rect.width(), TIMELINE_ROW_H),
                    );
                    let lane_bg = if row % 2 == 0 {
                        Color32::from_rgb(26, 28, 32)
                    } else {
                        Color32::from_rgb(30, 32, 36)
                    };
                    painter.rect_filled(row_rect, 2.0, lane_bg);
                    if !track_audible {
                        painter.rect_filled(row_rect, 2.0, Color32::from_rgba_premultiplied(0, 0, 0, 40));
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
                        Pos2::new(row_rect.left() + 6.0, row_rect.center().y - 7.0),
                        egui::Align2::LEFT_CENTER,
                        track_label,
                        egui::TextStyle::Small.resolve(ui.style()),
                        track_label_color,
                    );
                    for clip in &track.clips {
                        let x0 = outer_rect.left() + (clip.start_beat - loop_start) * beat_w;
                        let w = clip.length_beats * beat_w;
                        let clip_rect = Rect::from_min_size(
                            Pos2::new(x0, y),
                            Vec2::new(w.max(6.0), TIMELINE_ROW_H),
                        );
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
                        painter.text(
                            Pos2::new(clip_rect.left() + 4.0, clip_rect.top() + 2.0),
                            egui::Align2::LEFT_TOP,
                            clip.name.clone(),
                            egui::TextStyle::Small.resolve(ui.style()),
                            Color32::from_rgba_unmultiplied(240, 245, 250, 220),
                        );
                    }
                }

                let playhead = self.playhead_beat_display();
                let playhead_x = outer_rect.left() + (playhead - loop_start) * beat_w;
                if outer_rect.x_range().contains(playhead_x) {
                    painter.line_segment(
                        [
                            Pos2::new(playhead_x, ruler_rect.top()),
                            Pos2::new(playhead_x, outer_rect.bottom()),
                        ],
                        Stroke::new(2.0, Color32::from_rgb(255, 210, 90)),
                    );
                    painter.circle_filled(
                        Pos2::new(playhead_x, ruler_rect.top() + 8.0),
                        5.0,
                        Color32::from_rgb(255, 220, 100),
                    );
                }

                if outer_response.dragged() {
                    if let Some(pos) = outer_response.interact_pointer_pos() {
                        let lx = pos.x - outer_rect.left();
                        self.set_playhead_from_timeline(outer_rect, lx);
                        ui.ctx().request_repaint();
                    }
                }
                if outer_response.clicked() {
                    if let Some(pos) = outer_response.interact_pointer_pos() {
                        let lx = pos.x - outer_rect.left();
                        if ruler_rect.contains(pos) {
                            self.set_playhead_from_timeline(outer_rect, lx);
                        } else if pos.y >= ruler_rect.bottom() {
                            let mut hit: Option<(String, String)> = None;
                            'rows: for (row, track) in self.project.tracks.iter().enumerate() {
                                let y = tracks_rect.top()
                                    + 6.0
                                    + row as f32 * (TIMELINE_ROW_H + TIMELINE_ROW_GAP);
                                if !(y..=y + TIMELINE_ROW_H).contains(&pos.y) {
                                    continue;
                                }
                                for clip in &track.clips {
                                    let x0 = outer_rect.left()
                                        + (clip.start_beat - loop_start) * beat_w;
                                    let w = clip.length_beats * beat_w;
                                    let clip_rect = Rect::from_min_size(
                                        Pos2::new(x0, y),
                                        Vec2::new(w.max(6.0), TIMELINE_ROW_H),
                                    );
                                    if clip_rect.contains(pos) {
                                        hit = Some((track.id.clone(), clip.id.clone()));
                                        break 'rows;
                                    }
                                }
                            }
                            if let Some((tid, cid)) = hit {
                                self.selected_track = Some(tid);
                                self.selected_clip = Some(cid);
                            } else {
                                self.set_playhead_from_timeline(outer_rect, lx);
                            }
                        }
                    }
                }
            });
        });
    }

    fn piano_roll(&mut self, ui: &mut egui::Ui, panel_h: f32) {
        ui.set_min_height(panel_h);
        ui.set_max_height(panel_h);
        ui.vertical(|ui| {
            ui.heading("Piano Roll");
            match self.selected_clip_data() {
                None => {
                    let inner_h = ui.available_height().max(24.0);
                    egui::ScrollArea::vertical()
                        .id_salt("piano_roll_empty_scroll")
                        .max_height(inner_h)
                        .show(ui, |ui| {
                            ui.label("Select a clip");
                        });
                }
                Some((track_id, clip_id, clip)) => {
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

                    let inner_h = ui.available_height().max(24.0);
                    let viewport_w = ui.available_width();
                    let clip_len = clip.length_beats.max(1.0);
                    let grid_content_w =
                        (clip_len * TIMELINE_PX_PER_BEAT).max(viewport_w - PIANO_KEYS_W);
                    let pitch_min = 36_u8;
                    let pitch_max = 84_u8;
                    let pitch_span = (pitch_max - pitch_min) as f32;
                    let grid_body_h = (pitch_span * 7.0_f32).max(160.0);
                    let total_h = PIANO_RULER_H + grid_body_h;

                    egui::ScrollArea::both()
                        .id_salt("piano_roll_scroll")
                        .max_width(viewport_w)
                        .max_height(inner_h)
                        .show(ui, |ui| {
                            ui.horizontal_top(|ui| {
                                let (keys_rect, _) = ui.allocate_exact_size(
                                    Vec2::new(PIANO_KEYS_W, total_h),
                                    Sense::hover(),
                                );
                                let keys_painter = ui.painter_at(keys_rect);
                                keys_painter.rect_filled(keys_rect, 2.0, Color32::from_rgb(26, 28, 32));
                                keys_painter.line_segment(
                                    [
                                        Pos2::new(keys_rect.right(), keys_rect.top()),
                                        Pos2::new(keys_rect.right(), keys_rect.bottom()),
                                    ],
                                    Stroke::new(1.0, Color32::from_rgb(55, 58, 65)),
                                );

                                let row_h = grid_body_h / pitch_span;

                                let ruler_keys = Rect::from_min_size(
                                    keys_rect.min,
                                    Vec2::new(keys_rect.width(), PIANO_RULER_H),
                                );
                                keys_painter.rect_filled(ruler_keys, 0.0, Color32::from_rgb(32, 34, 40));

                                for p in pitch_min..=pitch_max {
                                    let row_from_bottom = (p - pitch_min) as f32;
                                    let y_cell_bottom = keys_rect.bottom() - row_from_bottom * row_h;
                                    let y_cell_top = y_cell_bottom - row_h;
                                    let cell = Rect::from_min_max(
                                        Pos2::new(keys_rect.left(), y_cell_top),
                                        Pos2::new(keys_rect.right(), y_cell_bottom),
                                    );
                                    let black = matches!(p % 12, 1 | 3 | 6 | 8 | 10);
                                    if black {
                                        keys_painter.rect_filled(cell, 0.0, Color32::from_rgb(34, 36, 40));
                                    }
                                    keys_painter.text(
                                        cell.left_center() + Vec2::new(4.0, 0.0),
                                        egui::Align2::LEFT_CENTER,
                                        midi_note_name(p),
                                        egui::TextStyle::Small.resolve(ui.style()),
                                        if black {
                                            Color32::from_rgb(190, 195, 205)
                                        } else {
                                            Color32::from_rgb(215, 218, 225)
                                        },
                                    );
                                }

                                let (grid_rect, _) = ui.allocate_exact_size(
                                    Vec2::new(grid_content_w, total_h),
                                    Sense::click(),
                                );
                                let painter = ui.painter_at(grid_rect);
                                painter.rect_filled(grid_rect, 2.0, Color32::from_rgb(20, 22, 25));

                                let ruler_rect = Rect::from_min_size(
                                    grid_rect.min,
                                    Vec2::new(grid_rect.width(), PIANO_RULER_H),
                                );
                                let body_rect = Rect::from_min_size(
                                    Pos2::new(grid_rect.left(), ruler_rect.bottom()),
                                    Vec2::new(grid_rect.width(), grid_body_h),
                                );
                                painter.rect_filled(ruler_rect, 0.0, Color32::from_rgb(30, 32, 38));
                                painter.line_segment(
                                    [
                                        Pos2::new(grid_rect.left(), ruler_rect.bottom()),
                                        Pos2::new(grid_rect.right(), ruler_rect.bottom()),
                                    ],
                                    Stroke::new(1.0, Color32::from_rgb(55, 58, 65)),
                                );

                                let beat_w = body_rect.width() / clip_len;
                                for sixteenth in 0..=(clip_len * 4.0).ceil() as i32 {
                                    let t = sixteenth as f32 / 4.0;
                                    if t > clip_len + 0.001 {
                                        break;
                                    }
                                    let x = body_rect.left() + t * beat_w;
                                    let is_beat = sixteenth % 4 == 0;
                                    let is_bar = sixteenth % 16 == 0;
                                    let (w, c) = if is_bar {
                                        (1.5, Color32::from_rgb(100, 108, 120))
                                    } else if is_beat {
                                        (1.0, Color32::from_rgb(55, 60, 70))
                                    } else {
                                        (1.0, Color32::from_rgb(40, 44, 52))
                                    };
                                    painter.line_segment(
                                        [
                                            Pos2::new(x, ruler_rect.top()),
                                            Pos2::new(x, grid_rect.bottom()),
                                        ],
                                        Stroke::new(w, c),
                                    );
                                }

                                for beat_i in 0..=clip_len.ceil() as usize {
                                    let t = beat_i as f32;
                                    let x = body_rect.left() + t * beat_w;
                                    let bar_in_clip = ((t / BEATS_PER_BAR).floor() as i32) + 1;
                                    let beat_in_bar = ((t % BEATS_PER_BAR).floor() as i32) + 1;
                                    painter.text(
                                        Pos2::new(x + 2.0, ruler_rect.top() + 3.0),
                                        egui::Align2::LEFT_TOP,
                                        format!("{bar_in_clip}.{beat_in_bar}"),
                                        egui::TextStyle::Small.resolve(ui.style()),
                                        Color32::from_rgb(175, 180, 190),
                                    );
                                }

                                for p in pitch_min..=pitch_max {
                                    let row_from_bottom = (p - pitch_min) as f32;
                                    let y = body_rect.bottom() - row_from_bottom * row_h;
                                    let black = matches!(p % 12, 1 | 3 | 6 | 8 | 10);
                                    if black {
                                        painter.rect_filled(
                                            Rect::from_min_max(
                                                Pos2::new(body_rect.left(), y - row_h),
                                                Pos2::new(body_rect.right(), y),
                                            ),
                                            0.0,
                                            Color32::from_rgb(24, 26, 30),
                                        );
                                    }
                                    painter.line_segment(
                                        [
                                            Pos2::new(body_rect.left(), y),
                                            Pos2::new(body_rect.right(), y),
                                        ],
                                        Stroke::new(1.0, Color32::from_rgb(38, 42, 48)),
                                    );
                                }

                                for note in &clip.notes {
                                    if note.pitch < pitch_min || note.pitch > pitch_max {
                                        continue;
                                    }
                                    let x = body_rect.left() + note.start_beat * beat_w;
                                    let y_bottom = body_rect.bottom()
                                        - (note.pitch - pitch_min) as f32 * row_h;
                                    let vel_h = row_h * (0.28 + 0.72 * note.velocity.clamp(0.0, 1.0));
                                    let y_top = y_bottom - vel_h;
                                    let note_rect = Rect::from_min_size(
                                        Pos2::new(x, y_top),
                                        Vec2::new(
                                            (note.length_beats * beat_w).max(3.0),
                                            vel_h.max(3.0),
                                        ),
                                    );
                                    let base = Color32::from_rgb(210, 132, 74);
                                    let top = Color32::from_rgb(245, 188, 120);
                                    painter.rect_filled(note_rect, 2.0, base);
                                    painter.line_segment(
                                        [
                                            note_rect.left_top() + Vec2::new(0.0, 2.0),
                                            note_rect.right_top() + Vec2::new(0.0, 2.0),
                                        ],
                                        Stroke::new(2.0, top),
                                    );
                                }

                                let clip_playhead = self.playhead_beat_display() - clip.start_beat;
                                if (-0.05..=clip_len + 0.05).contains(&clip_playhead) {
                                    let playhead_x = body_rect.left() + clip_playhead * beat_w;
                                    painter.line_segment(
                                        [
                                            Pos2::new(playhead_x, ruler_rect.top()),
                                            Pos2::new(playhead_x, grid_rect.bottom()),
                                        ],
                                        Stroke::new(2.0, Color32::from_rgb(255, 210, 90)),
                                    );
                                    painter.circle_filled(
                                        Pos2::new(playhead_x, ruler_rect.top() + 8.0),
                                        5.0,
                                        Color32::from_rgb(255, 220, 100),
                                    );
                                }
                            });
                        });
                }
            }
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

    fn new_project(&mut self) {
        self.audio.stop();
        self.project = Project::blank();
        self.undo.clear();
        self.project_path = default_project_path().display().to_string();
        self.ensure_selection();
        self.transport_playhead_beat = self.project.loop_start_beat();
        self.status = "New project".to_owned();
    }

    fn open_project_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Project manifest (project.json)", &["json"])
            .set_directory(default_open_dir())
            .pick_file()
        else {
            self.status = "Open canceled".to_owned();
            return;
        };
        if path.file_name().and_then(|n| n.to_str()) != Some(project_io::PROJECT_MANIFEST) {
            self.status = format!(
                "Please select a file named {}",
                project_io::PROJECT_MANIFEST
            );
            return;
        }
        self.run_io(|this| this.load_project_from_path(&path));
    }

    fn save_project_dialog(&mut self) {
        let Some(dir) = rfd::FileDialog::new()
            .set_directory(default_open_dir())
            .pick_folder()
        else {
            self.status = "Save canceled".to_owned();
            return;
        };
        let manifest = dir.join(project_io::PROJECT_MANIFEST);
        self.project_path = manifest.display().to_string();
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
        let path = Path::new(&self.project_path);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
        }
        project_io::save_project(&self.project, path)
            .with_context(|| format!("saving {}", self.project_path))
    }

    fn load_project(&mut self) -> Result<()> {
        let path = Path::new(&self.project_path);
        self.audio.stop();
        self.project = project_io::load_project(path)
            .with_context(|| format!("loading {}", self.project_path))?;
        self.undo.clear();
        self.ensure_selection();
        self.transport_playhead_beat = self.project.loop_start_beat();
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
                self.audio
                    .play_from_beat(&self.project, looping, self.transport_playhead_beat)?;
                self.loop_playback = looping;
                self.status = "Playing".to_owned();
                Ok(json!({"playing": true, "looping": looping}))
            }
            ControlRequest::Stop => {
                if self.audio.is_playing() {
                    self.transport_playhead_beat = self.playhead_beat_from_audio();
                }
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
        .join("music-rs-projects")
        .join("Untitled")
        .join(project_io::PROJECT_MANIFEST)
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
