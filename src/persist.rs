//! Where a diagram's layout (positions, open/closed state, view toggles) is remembered:
//! next to the file as `<file>.layout.json`, or (in the browser) in localStorage by file name.

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::path::PathBuf;

use crate::scene::SavedLayout;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutKey {
    /// A file on disk: its sidecar.
    Path(PathBuf),
    /// An in-memory diagram, by file name.
    Name(String),
}

pub fn load(key: &LayoutKey) -> Option<SavedLayout> {
    serde_json::from_str(&read(key)?).ok()
}

pub fn save(key: &LayoutKey, layout: &SavedLayout) -> Result<(), String> {
    let json = serde_json::to_string_pretty(layout).map_err(|e| e.to_string())?;
    write(key, &json)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn sidecar_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".layout.json");
    PathBuf::from(s)
}

#[cfg(not(target_arch = "wasm32"))]
fn read(key: &LayoutKey) -> Option<String> {
    match key {
        LayoutKey::Path(p) => std::fs::read_to_string(sidecar_path(p)).ok(),
        LayoutKey::Name(_) => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn write(key: &LayoutKey, json: &str) -> Result<(), String> {
    match key {
        LayoutKey::Path(p) => std::fs::write(sidecar_path(p), json).map_err(|e| e.to_string()),
        LayoutKey::Name(_) => Ok(()),
    }
}

#[cfg(target_arch = "wasm32")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

#[cfg(target_arch = "wasm32")]
fn storage_key(key: &LayoutKey) -> String {
    match key {
        LayoutKey::Name(n) => format!("ernuance.layout.{n}"),
        LayoutKey::Path(p) => format!("ernuance.layout.{}", p.display()),
    }
}

#[cfg(target_arch = "wasm32")]
fn read(key: &LayoutKey) -> Option<String> {
    storage()?.get_item(&storage_key(key)).ok()?
}

#[cfg(target_arch = "wasm32")]
fn write(key: &LayoutKey, json: &str) -> Result<(), String> {
    // Private browsing or a full quota: report, never crash.
    let s = storage().ok_or("browser storage is not available")?;
    s.set_item(&storage_key(key), json)
        .map_err(|_| "browser storage is full or blocked".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grouping::GroupingMode;
    use crate::mermaid::parse;
    use crate::scene::{Scene, tests::CharMeasure};

    #[test]
    fn sidecar_round_trip_and_names_are_not_stored_natively() {
        let dir = tempfile::tempdir().unwrap();
        let key = LayoutKey::Path(dir.path().join("s.mmd"));
        assert!(load(&key).is_none());
        let layout = SavedLayout {
            descriptions: true,
            tags: vec!["x".into()],
            ..Default::default()
        };
        save(&key, &layout).unwrap();
        assert!(dir.path().join("s.mmd.layout.json").is_file());
        let back = load(&key).unwrap();
        assert!(back.descriptions && back.tags == ["x"]);
        // In-memory sides have no place to store a layout on the desktop.
        save(&LayoutKey::Name("pasted.mmd".into()), &layout).unwrap();
        assert!(load(&LayoutKey::Name("pasted.mmd".into())).is_none());
    }

    #[test]
    fn a_layout_of_another_diagram_is_harmless() {
        // Same file name, different diagram (likely in the browser): unknown ids are ignored and
        // entities without a saved position are placed.
        let other = Scene::new(
            parse("erDiagram\nX ||--o{ Y : r\n"),
            GroupingMode::None,
            None,
            &CharMeasure,
        );
        let saved = other.to_saved([0.0, 0.0, 1.0]);
        let s = Scene::new(
            parse("erDiagram\nA ||--o{ B : r\nB ||--o{ C : s\n"),
            GroupingMode::None,
            Some(&saved),
            &CharMeasure,
        );
        assert_eq!(s.ents.len(), 3);
        let r: Vec<_> = (0..3)
            .map(|e| s.entity_rect(e, crate::scene::Which::Target))
            .collect();
        assert!(!r[0].intersects(r[1]) && !r[1].intersects(r[2]) && !r[0].intersects(r[2]));
    }
}
