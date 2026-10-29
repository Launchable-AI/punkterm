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
    /// The rail footer's PLAN and LOAD panels are folded to a line each.
    pub plan_collapsed: Option<bool>,
    pub load_collapsed: Option<bool>,
    pub groups: Vec<GroupPref>,
}

/// A session group as saved.
#[derive(Clone, Debug)]
pub struct GroupPref {
    pub name: String,
    /// Rolled up: its sessions hidden.
    pub collapsed: bool,
    /// Its cards one line each.
    pub brief: bool,
    /// Host name, tab, tmux pane.
    pub members: Vec<String>,
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
        plan_collapsed: get("plan_collapsed").and_then(|v| v.as_bool()),
        load_collapsed: get("load_collapsed").and_then(|v| v.as_bool()),
        groups: get("groups")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|g| {
                let t = g.as_table()?;
                let name = t.get("name")?.as_str()?.to_string();
                let flag = |k: &str| t.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
                let members = t.get("members")?.as_array()?.iter().filter_map(|m| m.as_str().map(str::to_string)).collect();
                Some(GroupPref { name, collapsed: flag("collapsed"), brief: flag("brief"), members })
            })
            .collect(),
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
    for (k, v) in [("plan_collapsed", p.plan_collapsed), ("load_collapsed", p.load_collapsed)] {
        if let Some(v) = v {
            t.insert(k.into(), v.into());
        }
    }
    if !p.groups.is_empty() {
        let groups = p.groups.iter().map(|gp| {
            let mut g = toml::Table::new();
            g.insert("name".into(), gp.name.clone().into());
            g.insert("collapsed".into(), gp.collapsed.into());
            g.insert("brief".into(), gp.brief.into());
            g.insert("members".into(), toml::Value::Array(gp.members.iter().map(|m| m.clone().into()).collect()));
            toml::Value::Table(g)
        });
        t.insert("groups".into(), toml::Value::Array(groups.collect()));
    }
    let _ = std::fs::write(path, toml::to_string(&t).unwrap_or_default());
}
