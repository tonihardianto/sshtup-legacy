mod app;
mod autocomplete;
mod card;
mod collab;
mod collab_net;
mod hostcard;
mod icons;
mod lock;
mod model;
mod relay_join;
mod typing_osc;
mod schemes;
mod screens;
mod sftp;
mod sync;
mod term;
mod ui;
mod vaultcrypt;
mod wire;

use eframe::egui;

/// Shown in the window title.
const APP_NAME: &str = "SSHTUP";

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    // Guest of a shared terminal: `--relay-join <host:port> <code~key> <name> <ticket>` (run inside a terminal tab).
    if args.get(1).map(|a| a == "--relay-join").unwrap_or(false) {
        let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
        let name = if arg(4).is_empty() { "guest" } else { arg(4) };
        std::process::exit(relay_join::run(arg(2), arg(3), name, arg(5)));
    }
    let mut shot: Option<String> = None;
    let mut start_screen: Option<String> = None;
    let mut open_local = false;
    let mut open_host: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--screenshot" => {
                shot = args.get(i + 1).cloned();
                i += 1;
            }
            "--screen" => {
                start_screen = args.get(i + 1).cloned();
                i += 1;
            }
            "--open-local" => open_local = true,
            "--open-host" => {
                open_host = args.get(i + 1).cloned();
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_inner_size([1180.0, 740.0])
        .with_min_inner_size([720.0, 440.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    if shot.is_some() {
        // A check run must never steal focus from what the person is doing.
        viewport = viewport.with_active(false);
        #[cfg(target_os = "windows")]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions { viewport, ..Default::default() };
    #[cfg(target_os = "macos")]
    if shot.is_some() {
        options.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            b.with_activate_ignoring_other_apps(false);
        }));
    }

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, shot, start_screen, open_local, open_host)))),
    )
}
