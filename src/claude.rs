//! Read a Claude Code session's state straight off its screen. The rules
//! mirror cc-rail's collector: a permission prompt or a turning spinner near
//! the bottom of the content says what the session is doing.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Shell,
    Idle,
    Working,
    YourTurn,
    Blocked,
}

/// What one look at the screen says.
pub struct Reading {
    pub claude: bool,
    pub blocked: bool,
    pub working: bool,
    /// The spinner line's verb, e.g. "Reading src/ui.rs…".
    pub activity: Option<String>,
    /// The session's own background work, e.g. "1 dynamic workflow" or
    /// "1 shell", and whether any of it is agents or workflows (which make
    /// the session busy; a shell alone may be a server left running).
    pub background: Option<String>,
    pub background_busy: bool,
}

/// Claude Code prefixes its title with a status glyph.
pub fn titled_claude(title: &str) -> bool {
    title.chars().next().is_some_and(|c| "✨✳✻✽✴✶✹✸·•●○⠁⠂⠄⡀⢀⠠⠐⠈⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(c))
}

pub fn read<T>(term: &Term<T>, title: &str) -> Reading {
    let grid = term.grid();
    let (cols, lines) = (term.columns(), term.screen_lines());
    let mut rows: Vec<String> = Vec::with_capacity(lines);
    for l in 0..lines {
        let row = &grid[Line(l as i32)];
        let s: String = (0..cols).map(|c| row[Column(c)].c).collect();
        rows.push(s.trim_end().to_string());
    }
    while rows.last().is_some_and(|r| r.is_empty()) {
        rows.pop();
    }
    let tail = &rows[rows.len().saturating_sub(16)..];
    let blocked = tail.iter().any(|r| r.contains("Do you want") || r.contains("❯ 1. Yes"));
    let mut working = false;
    let mut activity = None;
    for r in tail {
        if r.contains("esc to interrupt") || spinner_timer(r) {
            working = true;
            let verb = r.split(" (").next().unwrap_or(r).trim();
            let verb = verb.trim_start_matches(|c: char| !c.is_alphanumeric()).trim();
            if !verb.is_empty() {
                activity = Some(verb.to_string());
            }
        }
    }
    let (background, background_busy) = background(&rows);
    let claude = titled_claude(title) || working || tail.iter().any(|r| r.contains("? for shortcuts"));
    Reading { claude, blocked, working, activity, background, background_busy }
}

/// What the session left running, as a short summary ("4 agents · 3
/// workflows · 8 shells") and whether any of it is agents or workflows.
///
/// - The task panel under the footer: "● main", then a "◯ name  doing…" row per
///   running agent and "◯ name  ▰▰▱▱  1/2 · …" per workflow.
/// - Failing that, "✻ Waiting for 1 background agent and 3 dynamic workflows
///   to finish".
/// - Footer counts between "·": "8 shells", "2 agents".
///
/// The footer's "← 4 agents" is every agent on the host (`claude agents`),
/// not this session's, so it is skipped.
fn background(rows: &[String]) -> (Option<String>, bool) {
    let plural = |n: usize, noun: &str| format!("{n} {noun}{}", if n == 1 { "" } else { "s" });
    let mut parts = Vec::new();
    let main = rows.iter().rposition(|r| r.trim() == "● main");
    if let Some(main) = main {
        let tasks: Vec<&String> = rows[main + 1..].iter().filter(|r| r.trim_start().starts_with('◯')).collect();
        let flows = tasks.iter().filter(|r| r.contains(['▰', '▱'])).count();
        if tasks.len() > flows {
            parts.push(plural(tasks.len() - flows, "agent"));
        }
        if flows > 0 {
            parts.push(plural(flows, "workflow"));
        }
    }
    if parts.is_empty() {
        // Only the live status line, just above the prompt; old ones stay
        // in the transcript after the work is done.
        let prompt = rows.iter().rposition(|r| r.trim_start().starts_with('❯')).unwrap_or(rows.len());
        let tail = &rows[prompt.saturating_sub(3)..prompt];
        if let Some(w) = tail.iter().rev().find_map(|r| r.split_once("Waiting for ").and_then(|(_, w)| w.split_once(" to finish")).map(|(w, _)| w.to_string())) {
            for item in w.split(" and ").flat_map(|x| x.split(", ")) {
                let words: Vec<&str> = item.split_whitespace().collect();
                if let (Some(n), Some(noun)) = (words.first().and_then(|n| n.parse::<usize>().ok()), words.last()) {
                    parts.push(plural(n, noun.trim_end_matches('s')));
                }
            }
        }
    }
    let busy = !parts.is_empty();
    // Footer counts: the input box's status row, just above the panel.
    let above: Vec<&String> = rows[..main.unwrap_or(rows.len())].iter().filter(|r| !r.trim().is_empty()).collect();
    let footer = &above[above.len().saturating_sub(2)..];
    let mut busy_footer = false;
    for seg in footer.iter().flat_map(|r| r.split(" · ")) {
        let seg = seg.trim();
        let Some((n, noun)) = seg.split_once(' ') else { continue };
        let Ok(n) = n.parse::<usize>() else { continue };
        match noun.trim_end_matches('s') {
            "shell" => parts.push(plural(n, "shell")),
            "agent" | "task" | "workflow" | "subagent" if !busy => {
                busy_footer = true;
                parts.push(seg.to_string());
            }
            _ => {}
        }
    }
    ((!parts.is_empty()).then(|| parts.join(" · ")), busy || busy_footer)
}

/// Matches "…(12s" / "… (1m 3s" — Claude's spinner timer.
fn spinner_timer(r: &str) -> bool {
    let Some(i) = r.find('…') else { return false };
    let rest = r[i + '…'.len_utf8()..].trim_start();
    let Some(rest) = rest.strip_prefix('(') else { return false };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit() || " hms".contains(*c)).collect();
    digits.trim_end().ends_with('s') && digits.chars().any(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner() {
        assert!(spinner_timer("✻ Pondering… (12s · ↑ 1.2k tokens · esc to interrupt)"));
        assert!(spinner_timer("· Reading… (1m 3s"));
        assert!(!spinner_timer("see the docs… (and more)"));
        assert!(!spinner_timer("no spinner here"));
    }

    #[test]
    fn background_work() {
        let rows = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let panel = rows(&[
            "✻ Waiting for 1 background agent and 3 dynamic workflows to finish",
            "❯ also start the release notes",
            "  -- INSERT -- ⏵⏵ auto mode on · 8 shells",
            "  ● main",
            "  ◯ general-purpose     Patching the parser in report.py",
            "  ◯ general-purpose     Fixing a date in NOTES.md",
            "  ◯ code-review         ▰▰▰▰▰▰▱▱▱▱  1/2 · 15m39s · ↓ 250.0k tokens",
            "  ◯ test-matrix         ▰▰▰▰▰▰▰▱▱▱  11/16 · 15m39s · ↓ 993.9k tokens",
        ]);
        assert_eq!(background(&panel), (Some("2 agents · 2 workflows · 8 shells".into()), true));
        let waiting = rows(&["✻ Waiting for 1 background agent and 3 dynamic workflows to finish", "────", "❯ ", "────", "  -- INSERT -- auto mode on"]);
        assert_eq!(background(&waiting), (Some("1 agent · 3 workflows".into()), true));
        let stale = rows(&["✻ Waiting for 1 dynamic workflow to finish", "● Done: all three finished.", "", "", "✻ Worked for 3s", "────", "❯ ", "────", "  -- INSERT -- auto mode on"]);
        assert_eq!(background(&stale), (None, false));
        let shell = rows(&["❯ cut the release", "  -- INSERT -- ⏵⏵ auto mode on · 1 shell · ← 4 agents          /rc"]);
        assert_eq!(background(&shell), (Some("1 shell".into()), false));
        let host = rows(&["❯ build it", "  -- INSERT -- ⏵⏵ auto mode on (shift+tab to cycle) · ← 4 agents · PR #42 · ⧉ 3"]);
        assert_eq!(background(&host), (None, false));
    }

    #[test]
    fn title() {
        assert!(titled_claude("✳ Refactor rail"));
        assert!(!titled_claude("zsh"));
    }
}
