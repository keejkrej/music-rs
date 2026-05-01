use std::path::Path;

use anyhow::Context;
use music_rs::app::DawApp;
use music_rs::control::{ControlServer, start_control_server};
use music_rs::midi_import;
use music_rs::project_io;

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return Ok(());
    }

    if args.first().is_some_and(|s| s == "midi-to-json") {
        let mid = args
            .get(1)
            .context("midi-to-json: missing input path (.mid / .midi)")?;
        let out = args
            .get(2)
            .context("midi-to-json: missing output path (.json or .../project.json)")?;
        return run_midi_to_json(Path::new(mid), Path::new(out));
    }

    let control_server = parse_control_server(&args)?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1180.0, 760.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Music RS",
        options,
        Box::new(|cc| Ok(Box::new(DawApp::new(cc, control_server)))),
    )
    .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok(())
}

fn print_usage() {
    println!(
        "\
usage:
  daw
  daw serve [<port>]
  daw midi-to-json <input.mid> <output.json|.../project.json>
  daw --help
"
    );
}

fn run_midi_to_json(mid: &Path, out: &Path) -> anyhow::Result<()> {
    let project = midi_import::import_midi_path(mid)?;
    if project_io::is_split_manifest_path(out) {
        project_io::save_project(&project, out)?;
    } else {
        if let Some(parent) = out.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(out, project.to_json()?)
            .with_context(|| format!("writing {}", out.display()))?;
    }
    Ok(())
}

fn parse_control_server(args: &[String]) -> anyhow::Result<Option<ControlServer>> {
    match args {
        [] => Ok(None),
        [mode, port] if mode == "serve" => {
            let port = port.parse::<u16>()?;
            let server = start_control_server(port)?;
            eprintln!("daw control listening on ws://{}", server.addr);
            Ok(Some(server))
        }
        [mode] if mode == "serve" => {
            let server = start_control_server(4141)?;
            eprintln!("daw control listening on ws://{}", server.addr);
            Ok(Some(server))
        }
        _ => anyhow::bail!(
            "usage: daw | daw serve [<port>] | daw midi-to-json <in.mid> <out.json> | daw --help"
        ),
    }
}
