use std::path::Path;

use music_rs::{
    ai::{RuleBasedProvider, validated_ai_edits},
    commands::{EditCommand, apply_commands},
    project::Project,
    project_io,
    render::export_wav,
    undo::{UndoStack, apply_undoable},
};

#[test]
fn ai_loop_can_be_edited_saved_loaded_and_exported() {
    let mut project = Project {
        tracks: Vec::new(),
        ..Project::default()
    };
    let mut undo = UndoStack::default();

    let commands = validated_ai_edits(
        &RuleBasedProvider,
        "make an 8 bar house loop at 124 BPM",
        &project,
    )
    .unwrap();
    apply_undoable(&mut project, &mut undo, |project| {
        apply_commands(project, commands)
    })
    .unwrap();
    assert_eq!(project.tempo_bpm, 124.0);
    assert_eq!(project.loop_bars, 8);
    assert!(project.tracks.iter().any(|track| !track.clips.is_empty()));

    let synth_id = project.create_track(
        "Lead",
        music_rs::project::Instrument::Synth {
            waveform: music_rs::project::Waveform::Saw,
        },
    );
    project.add_clip(&synth_id, "Lead Clip", 0.0, project.loop_length_beats());
    let clip_id = project.track(&synth_id).unwrap().clips[0].id.clone();
    apply_commands(
        &mut project,
        vec![EditCommand::AddNotes {
            track_id: synth_id,
            clip_id: Some(clip_id),
            notes: vec![music_rs::project::MidiNote {
                pitch: 64,
                velocity: 0.7,
                start_beat: 0.0,
                length_beats: 1.0,
            }],
        }],
    )
    .unwrap();

    let dir = std::env::temp_dir().join(format!(
        "music-rs-acceptance-io-{}",
        music_rs::project::new_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = dir.join(project_io::PROJECT_MANIFEST);
    project_io::save_project(&project, &manifest).unwrap();
    let loaded = project_io::load_project(&dir).unwrap();
    assert_eq!(loaded.loop_bars, 8);
    assert_eq!(loaded.tempo_bpm, 124.0);

    let path = std::env::temp_dir().join(format!(
        "music-rs-acceptance-{}.wav",
        music_rs::project::new_id()
    ));
    export_wav(&loaded, &path).unwrap();
    let reader = hound::WavReader::open(&path).unwrap();
    assert_eq!(reader.spec().channels, 2);
    assert!(reader.duration() > 0);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(path);
}

#[test]
fn bundled_example_project_loads_and_renders() {
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/projects/happy_birthday");
    let project = project_io::load_project(&sample).unwrap();
    assert!(!project.tracks.is_empty());
    let path = std::env::temp_dir().join(format!(
        "music-rs-bundled-{}.wav",
        music_rs::project::new_id()
    ));
    export_wav(&project, &path).unwrap();
    let reader = hound::WavReader::open(&path).unwrap();
    assert!(reader.duration() > 0);
    let _ = std::fs::remove_file(path);
}
