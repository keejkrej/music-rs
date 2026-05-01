use music_rs::app::DawApp;
use music_rs::control::{ControlServer, start_control_server};

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let control_server = parse_control_server()?;
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

fn parse_control_server() -> anyhow::Result<Option<ControlServer>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
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
        [flag] if flag == "--help" || flag == "-h" => {
            println!("usage: daw [serve <port>]");
            std::process::exit(0);
        }
        _ => anyhow::bail!("usage: daw [serve <port>]"),
    }
}
