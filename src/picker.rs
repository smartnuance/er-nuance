//! Choosing a file to open or compare with, and files dropped onto the window. Natively the
//! dialog blocks and a dropped file is known by its path, so results are ready at once; in the
//! browser both are read asynchronously and arrive a few frames later. Either way the app
//! collects results with [`Picker::take`] each frame.

use std::sync::{Arc, Mutex};

use crate::source::Side;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Open,
    /// Compare the picked file (old) with the open diagram (new).
    CompareWith,
}

#[derive(Clone, Debug)]
pub struct Picked {
    pub purpose: Purpose,
    pub side: Side,
}

#[derive(Clone, Default)]
pub struct Picker {
    ready: Arc<Mutex<Vec<Picked>>>,
}

#[cfg(not(target_arch = "wasm32"))]
const EXTENSIONS: &[&str] = &["mmd", "mermaid", "md", "markdown", "txt"];

/// A file's contents as an in-memory side. Bytes that are not UTF-8 become replacement
/// characters; the parser then reports what it cannot read.
#[cfg(any(test, target_arch = "wasm32"))]
pub fn text_side(name: &str, bytes: &[u8]) -> Side {
    let name = if name.is_empty() { "dropped.mmd" } else { name };
    Side::Text {
        name: name.to_owned(),
        text: String::from_utf8_lossy(bytes).into_owned(),
    }
}

impl Picker {
    pub fn deliver(&self, picked: Picked) {
        self.ready.lock().unwrap().push(picked);
    }

    pub fn take(&self) -> Vec<Picked> {
        std::mem::take(&mut *self.ready.lock().unwrap())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn pick(&self, purpose: Purpose, _ctx: &egui::Context) {
        let title = match purpose {
            Purpose::Open => "Open diagram",
            Purpose::CompareWith => "Compare with…",
        };
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("Mermaid ER", EXTENSIONS)
            .set_title(title)
            .pick_file()
        {
            self.deliver(Picked {
                purpose,
                side: Side::File(p),
            });
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn pick(&self, purpose: Purpose, ctx: &egui::Context) {
        let this = self.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let dialog = rfd::AsyncFileDialog::new()
                .add_filter("Mermaid ER", &["mmd", "mermaid", "md", "markdown", "txt"]);
            let Some(file) = dialog.pick_file().await else {
                return;
            };
            let side = text_side(&file.file_name(), &file.read().await);
            this.deliver(Picked { purpose, side });
            ctx.request_repaint();
        });
    }

    /// Reads a file dropped onto the page (browsers only allow reading it asynchronously).
    #[cfg(target_arch = "wasm32")]
    pub fn dropped(&self, file: &egui::DroppedFileHandle, ctx: &egui::Context) {
        let (this, ctx, file) = (self.clone(), ctx.clone(), file.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let name = file.path().to_string_lossy().into_owned();
            match file.bytes_async().await {
                Ok(bytes) => this.deliver(Picked {
                    purpose: Purpose::Open,
                    side: text_side(&name, &bytes),
                }),
                Err(e) => {
                    eframe::web_sys::console::error_1(&format!("cannot read {name}: {e}").into())
                }
            }
            ctx.request_repaint();
        });
    }

    /// Opens a file dropped onto the window (by path; drops without one are ignored).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn dropped(&self, file: &egui::DroppedFileHandle, _ctx: &egui::Context) {
        let path = file.path();
        if !path.as_os_str().is_empty() {
            self.deliver(Picked {
                purpose: Purpose::Open,
                side: Side::File(path.to_owned()),
            });
        }
    }
}
