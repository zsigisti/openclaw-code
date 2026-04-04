//! Lightweight persistent config for openclaw-code.
//! Stored at `~/.config/openclaw-code/config.json`.

use std::path::PathBuf;

use serde_json::{json, Value};

fn config_path() -> Option<PathBuf> {
    // Respect $XDG_CONFIG_HOME if set, otherwise fall back to ~/.config.
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("openclaw-code/config.json"))
}

pub fn load_last_model() -> Option<String> {
    let path = config_path()?;
    let bytes = std::fs::read(&path).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let s = v.get("model")?.as_str()?;
    if s.is_empty() { None } else { Some(s.to_string()) }
}

pub fn save_last_model(model: &str) {
    let Some(path) = config_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Load existing config so we don't clobber other keys.
    let mut store: serde_json::Map<String, Value> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    store.insert("model".to_string(), json!(model));

    let tmp = path.with_extension("json.tmp");
    if let Ok(rendered) = serde_json::to_string_pretty(&Value::Object(store)) {
        let _ = std::fs::write(&tmp, format!("{rendered}\n"));
        let _ = std::fs::rename(&tmp, &path);
    }
}
