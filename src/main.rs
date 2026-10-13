#![forbid(unsafe_code)]

mod app;
mod canvas;
mod cli;
mod diff;
mod grouping;
mod layout;
mod mermaid;
mod model;
#[cfg(target_os = "macos")]
mod native_menu;
mod palette;
mod richtext;
mod scene;
mod vcs;

fn main() -> eframe::Result {
    let launch = match cli::parse(std::env::args_os()) {
        Ok(Ok(launch)) => launch,
        Ok(Err(msg)) => {
            eprintln!("ernuance: {msg}");
            std::process::exit(2);
        }
        Err(e) => e.exit(),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1480.0, 920.0])
            .with_title("ER Navigator")
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "ER Navigator",
        options,
        Box::new(|cc| Ok(Box::new(app::ErApp::new(cc, launch)))),
    )
}
