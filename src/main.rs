#![forbid(unsafe_code)]

mod app;
mod canvas;
#[cfg(not(target_arch = "wasm32"))]
mod cli;
mod diff;
mod grouping;
mod layout;
mod mermaid;
mod model;
#[cfg(target_os = "macos")]
mod native_menu;
mod palette;
mod persist;
mod picker;
mod richtext;
mod scene;
mod source;
mod vcs;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let launch = match cli::parse(std::env::args_os()) {
        Ok(Ok(launch)) => launch,
        Ok(Err(msg)) => {
            eprintln!("ernuance: {msg}");
            std::process::exit(2);
        }
        Err(e) => e.exit(),
    };
    let initial = match launch {
        cli::Launch::View(path) => path.map(|p| source::Source::View(source::Side::File(p))),
        cli::Launch::Diff { old, new } => Some(source::Source::Diff { old, new }),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1480.0, 920.0])
            .with_title("ER Nuance")
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "ER Nuance",
        options,
        Box::new(|cc| Ok(Box::new(app::ErApp::new(cc, initial)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;
    wasm_bindgen_futures::spawn_local(async {
        let document = eframe::web_sys::window()
            .expect("window")
            .document()
            .expect("document");
        let canvas = document
            .get_element_by_id("the_canvas_id")
            .expect("canvas in index.html")
            .dyn_into::<eframe::web_sys::HtmlCanvasElement>()
            .expect("a canvas");
        let start = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(app::ErApp::new(cc, None)))),
            )
            .await;
        if let Some(loading) = document.get_element_by_id("loading_text") {
            match start {
                Ok(()) => loading.remove(),
                Err(e) => {
                    loading.set_inner_html(
                        "<p>ER Nuance could not start. See the browser console for details.</p>",
                    );
                    eframe::web_sys::console::error_1(&e);
                }
            }
        }
    });
}
