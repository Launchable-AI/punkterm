//! Remembered preferences (theme, opacity, sound, hosts), kept in punkterm's own
//! state file: ~/.local/state/punkterm/state.toml.

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Prefs {
    pub theme: Option<String>,
    pub opacity: Option<f32>,
    pub sound: Option<String>,
    /// ssh specs of hosts added from the connect prompt.
    pub hosts: Vec<String>,
    /// The timeline rail is expanded.
    pub timeline: Option<bool>,
}

fn path() -> Option<PathBuf> {
    dirs::state_dir().map(|d| d.join("punkterm").join("state.toml"))
}

pub fn load() -> Prefs {
    let doc = path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| s.parse::<toml::Table>().ok());
    let get = |k: &str| doc.as_ref().and_then(|d| d.get(k).cloned());
    Prefs {
        theme: get("theme").and_then(|v| v.as_str().map(str::to_string)),
        opacity: get("opacity").and_then(|v| v.as_float().map(|f| f as f32)),
        sound: get("sound").and_then(|v| v.as_str().map(str::to_string)),
        hosts: get("hosts")
            .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|h| h.as_str().map(str::to_string)).collect()))
            .unwrap_or_default(),
        timeline: get("timeline").and_then(|v| v.as_bool()),
    }
}

pub fn save(p: &Prefs) {
    let Some(path) = path() else { return };
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    let mut t = toml::Table::new();
    if let Some(v) = &p.theme {
        t.insert("theme".into(), v.clone().into());
    }
    if let Some(v) = p.opacity {
        t.insert("opacity".into(), (v as f64).into());
    }
    if let Some(v) = &p.sound {
        t.insert("sound".into(), v.clone().into());
    }
    if !p.hosts.is_empty() {
        t.insert("hosts".into(), toml::Value::Array(p.hosts.iter().map(|h| h.clone().into()).collect()));
    }
    if let Some(v) = p.timeline {
        t.insert("timeline".into(), v.into());
    }
    let _ = std::fs::write(path, toml::to_string(&t).unwrap_or_default());
}
