//! Hosts. punkterm joins the tmux already running on each host (see
//! `tmuxcc`); hosts come from ~/.config/punkterm/config.toml, which punkterm
//! only reads:
//!
//! ```toml
//! [[host]]
//! name = "ec2-a"                                   # label in the rail
//! ssh = "-i ~/.ssh/key.pem ubuntu@203.0.113.7"      # destination, or ssh args
//! color = "#ffe23a"                                 # optional
//! ```
//!
//! Hosts added from the connect prompt (⌃⇧N) or `--host` are remembered in
//! punkterm's state file instead.

use crate::theme::{hex, Rgba};
use crate::tmuxcc;

#[derive(Clone, Debug)]
pub struct Host {
    pub name: String,
    /// `None` for the local machine; else a destination or ssh arguments.
    pub ssh: Option<String>,
    pub color: Option<Rgba>,
}

impl Host {
    pub fn from_spec(spec: &str) -> Host {
        let spec = spec.trim();
        let spec = spec.strip_prefix("ssh ").map(str::trim).unwrap_or(spec);
        Host { name: tmuxcc::ssh_host_name(spec), ssh: Some(spec.to_string()), color: None }
    }
}

pub fn config_path() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("PUNKTERM_CONFIG") {
        return Some(p.into());
    }
    dirs::config_dir().map(|d| d.join("punkterm").join("config.toml"))
}

/// The local machine first, then every configured host, then `extra` specs
/// (remembered or from `--host`) not already listed.
pub fn load(extra: &[String]) -> Vec<Host> {
    let mut hosts = vec![Host { name: "local".into(), ssh: None, color: None }];
    let doc = config_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| s.parse::<toml::Table>().ok());
    for h in doc.as_ref().and_then(|d| d.get("host")).and_then(|v| v.as_array()).into_iter().flatten() {
        let Some(t) = h.as_table() else { continue };
        let Some(name) = t.get("name").and_then(|v| v.as_str()) else { continue };
        let ssh = t.get("ssh").and_then(|v| v.as_str()).unwrap_or(name).to_string();
        let color = t.get("color").and_then(|v| v.as_str()).and_then(hex);
        hosts.push(Host { name: name.to_string(), ssh: Some(ssh), color });
    }
    for spec in extra {
        let h = Host::from_spec(spec);
        if !hosts.iter().any(|o| o.ssh == h.ssh) {
            hosts.push(h);
        }
    }
    hosts
}

/// Recent `ssh …` commands from shell history (read-only), most used first,
/// as suggestions for the connect prompt. Tunnels and git remotes are left
/// out: they are not hosts with tmux on them.
pub fn ssh_history() -> Vec<String> {
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    let mut seen: Vec<(String, usize, usize)> = Vec::new();
    let mut order = 0;
    for f in [".zsh_history", ".bash_history"] {
        let Ok(bytes) = std::fs::read(home.join(f)) else { continue };
        for line in String::from_utf8_lossy(&bytes).lines() {
            // zsh extended history: ": 1712345678:0;command"
            let cmd = match line.strip_prefix(": ") {
                Some(r) => r.split_once(';').map_or(r, |(_, c)| c),
                None => line,
            };
            let cmd = cmd.split(['|', ';', '&']).next().unwrap_or("").trim();
            let Some(rest) = cmd.strip_prefix("ssh ") else { continue };
            let words: Vec<&str> = rest.split_whitespace().collect();
            if words.is_empty() || words.iter().any(|w| matches!(*w, "-L" | "-R" | "-D" | "-N" | "-T" | "-W") || w.starts_with("git@")) {
                continue;
            }
            let spec = words.join(" ");
            order += 1;
            match seen.iter_mut().find(|(s, ..)| *s == spec) {
                Some(e) => {
                    e.1 += 1;
                    e.2 = order;
                }
                None => seen.push((spec, 1, order)),
            }
        }
    }
    seen.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    seen.into_iter().map(|(s, ..)| s).take(40).collect()
}
