//! Colors and fonts. punkterm ships its own palettes and also reads themer's
//! theme files (read-only) so it can follow the rest of the desktop.

use std::path::PathBuf;

pub type Rgba = [f32; 4];

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    pub bg: Rgba,
    pub fg: Rgba,
    pub cursor: Rgba,
    pub selection_bg: Rgba,
    pub ansi: [Rgba; 16],
    pub accent: Rgba,
    pub accent2: Rgba,
    pub warning: Rgba,
    pub error: Rgba,
    pub ok: Rgba,
    pub surface: Rgba,
    pub surface_alt: Rgba,
    pub border: Rgba,
    pub muted: Rgba,
    /// Session states. Kept apart from the accents so a blocked session
    /// stands out in every theme, monochrome ones included.
    pub alert: Rgba,
    pub busy: Rgba,
    pub ready: Rgba,
    pub mono_font: String,
    pub ui_font: String,
    /// Terminal font size in logical pixels.
    pub mono_px: f32,
}

pub fn hex(s: &str) -> Option<Rgba> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 && s.len() != 8 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    let a = if s.len() == 8 { p(6)? } else { 1.0 };
    Some([p(0)?, p(2)?, p(4)?, a])
}

pub fn with_alpha(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], c[3] * a]
}

pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

const ANSI_KEYS: [&str; 16] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    "bright_black", "bright_red", "bright_green", "bright_yellow", "bright_blue",
    "bright_magenta", "bright_cyan", "bright_white",
];

/// punkterm's own palettes, in cycling order.
pub const BUILTINS: [&str; 8] = [
    "neon", "matrix", "tron", "green-black", "blue-black", "amber", "ghost", "burnt-orange-light",
];

struct Spec {
    bg: &'static str,
    surface: &'static str,
    surface_alt: &'static str,
    border: &'static str,
    fg: &'static str,
    muted: &'static str,
    accent: &'static str,
    accent2: &'static str,
    cursor: &'static str,
    selection: &'static str,
    alert: &'static str,
    busy: &'static str,
    ready: &'static str,
    warning: &'static str,
    ansi: [&'static str; 16],
}

fn spec(name: &str) -> Option<Spec> {
    Some(match name {
        "neon" => Spec {
            bg: "#08080f", surface: "#0f0f19", surface_alt: "#181826", border: "#2a1a44",
            fg: "#dcdcf5", muted: "#6a6a96", accent: "#ff2a6d", accent2: "#00e5ff",
            cursor: "#ff2a6d", selection: "#3a1a4f",
            alert: "#ff2a6d", busy: "#00e5ff", ready: "#05ffa1", warning: "#ffe23a",
            ansi: [
                "#14141f", "#ff2a6d", "#05ffa1", "#ffe23a", "#01cdfe", "#b967ff", "#00fff5", "#dcdcf5",
                "#5a5a8a", "#ff6e9c", "#5cffb5", "#fff07a", "#5ce0ff", "#d49fff", "#7afff9", "#ffffff",
            ],
        },
        "matrix" => Spec {
            bg: "#010603", surface: "#040d07", surface_alt: "#08170d", border: "#0f3a1e",
            fg: "#b8ffcf", muted: "#3c7a51", accent: "#00ff66", accent2: "#00c98d",
            cursor: "#00ff66", selection: "#0f3a1e",
            alert: "#ff3b5c", busy: "#39ffb4", ready: "#c6ff3a", warning: "#d4ff3a",
            ansi: [
                "#04120a", "#ff3b5c", "#00ff66", "#c6ff3a", "#00c98d", "#5cffa8", "#39ffb4", "#b8ffcf",
                "#2f6b45", "#ff7088", "#66ff9e", "#dcff7a", "#3cf0b8", "#8effc4", "#7affd2", "#eafff1",
            ],
        },
        "tron" => Spec {
            bg: "#01040a", surface: "#040a15", surface_alt: "#091424", border: "#12305a",
            fg: "#d2ebff", muted: "#4d6d94", accent: "#0a9dff", accent2: "#6ff3ff",
            cursor: "#6ff3ff", selection: "#12305a",
            alert: "#ff8a00", busy: "#6ff3ff", ready: "#7dffcf", warning: "#ffd36a",
            ansi: [
                "#071224", "#ff5a5a", "#4dffb0", "#ffd36a", "#0a9dff", "#8a7dff", "#6ff3ff", "#d2ebff",
                "#3d5a80", "#ff8a8a", "#85ffcb", "#ffe39a", "#5cbcff", "#b0a8ff", "#a8f8ff", "#ffffff",
            ],
        },
        // Classic hacker terminals: pure black, phosphor green / electric blue.
        "green-black" => Spec {
            bg: "#000000", surface: "#000000", surface_alt: "#051109", border: "#0d331a",
            fg: "#33ff66", muted: "#1f8a3e", accent: "#00ff41", accent2: "#00b82e",
            cursor: "#00ff41", selection: "#0d4a22",
            alert: "#ff3b3b", busy: "#00ff41", ready: "#b6ff3a", warning: "#e8ff3a",
            ansi: [
                "#000000", "#ff3b3b", "#00ff41", "#b6ff3a", "#00c853", "#3dff8a", "#5cffb0", "#33ff66",
                "#1a6b33", "#ff6e6e", "#66ff8c", "#d4ff7a", "#33e07a", "#80ffb0", "#99ffd0", "#ccffd9",
            ],
        },
        "blue-black" => Spec {
            bg: "#000000", surface: "#000000", surface_alt: "#040c18", border: "#0d2a52",
            fg: "#4dc3ff", muted: "#2a6a99", accent: "#1e90ff", accent2: "#00e5ff",
            cursor: "#00e5ff", selection: "#0d2f5c",
            alert: "#ff6a3d", busy: "#00e5ff", ready: "#7dffd4", warning: "#ffd36a",
            ansi: [
                "#000000", "#ff5a5a", "#4dffb0", "#ffd36a", "#1e90ff", "#8a7dff", "#00e5ff", "#4dc3ff",
                "#24507a", "#ff8a8a", "#85ffcb", "#ffe39a", "#5cb0ff", "#b0a8ff", "#80f2ff", "#c8ecff",
            ],
        },
        "amber" => Spec {
            bg: "#090500", surface: "#110a02", surface_alt: "#1b1105", border: "#3d2808",
            fg: "#ffd9a0", muted: "#8a6838", accent: "#ffb000", accent2: "#ff7a1a",
            cursor: "#ffb000", selection: "#3d2808",
            alert: "#ff3b3b", busy: "#ffb000", ready: "#e8ff6a", warning: "#ffe08a",
            ansi: [
                "#1a1004", "#ff4f3b", "#d8ff6a", "#ffb000", "#ff8f3a", "#ff6f6f", "#ffc96a", "#ffd9a0",
                "#6e5228", "#ff7f6e", "#e8ff9a", "#ffcf5a", "#ffae6e", "#ff9a9a", "#ffe0a0", "#fff2dc",
            ],
        },
        "ghost" => Spec {
            bg: "#050505", surface: "#0c0c0d", surface_alt: "#161618", border: "#2c2c30",
            fg: "#e6e6e8", muted: "#6c6c72", accent: "#ffffff", accent2: "#a0a0a8",
            cursor: "#ffffff", selection: "#2c2c30",
            alert: "#ff3355", busy: "#e6e6e8", ready: "#9affc9", warning: "#ffe08a",
            ansi: [
                "#141416", "#ff5c6c", "#9affc9", "#ffe08a", "#9ab8ff", "#d0a8ff", "#a8f0ff", "#e6e6e8",
                "#55555c", "#ff8a96", "#bfffdc", "#fff0b8", "#bfd2ff", "#e2c8ff", "#ccf7ff", "#ffffff",
            ],
        },
        // Light: warm cream and brown with a burnt-orange accent, after the
        // wezterm burnt_orange_light palette.
        "burnt-orange-light" => Spec {
            bg: "#eadbc8", surface: "#fbf1e6", surface_alt: "#f0e4d6", border: "#d4a878",
            fg: "#3d2315", muted: "#8a7565", accent: "#c06820", accent2: "#3a6878",
            cursor: "#d08040", selection: "#e8c9a8",
            alert: "#b04030", busy: "#3a6878", ready: "#5a7038", warning: "#a07020",
            ansi: [
                "#3d2315", "#b04030", "#5a7038", "#a07020", "#3a6878", "#885060", "#387868", "#5c4435",
                "#6a5545", "#c85040", "#6a8040", "#b88030", "#4a7888", "#a06878", "#488878", "#1a1210",
            ],
        },
        _ => return None,
    })
}

impl Theme {
    pub fn builtin(name: &str) -> Option<Self> {
        let s = spec(name)?;
        let h = |v: &str| hex(v).unwrap();
        let mut ansi = [[0.0; 4]; 16];
        for (i, c) in s.ansi.iter().enumerate() {
            ansi[i] = h(c);
        }
        Some(Theme {
            name: name.to_string(),
            bg: h(s.bg),
            fg: h(s.fg),
            cursor: h(s.cursor),
            selection_bg: h(s.selection),
            ansi,
            accent: h(s.accent),
            accent2: h(s.accent2),
            warning: h(s.warning),
            error: h(s.alert),
            ok: h(s.ready),
            surface: h(s.surface),
            surface_alt: h(s.surface_alt),
            border: h(s.border),
            muted: h(s.muted),
            alert: h(s.alert),
            busy: h(s.busy),
            ready: h(s.ready),
            mono_font: "JetBrainsMono Nerd Font".into(),
            ui_font: "JetBrainsMono Nerd Font Propo".into(),
            mono_px: 17.0,
        })
    }

    pub fn themer_dir() -> Option<PathBuf> {
        crate::state::config_dir().map(|d| d.join("themer").join("themes"))
    }

    /// Every theme punkterm can cycle through: its own, then themer's dark ones.
    pub fn names() -> Vec<String> {
        let mut v: Vec<String> = BUILTINS.iter().map(|s| s.to_string()).collect();
        let mut themer: Vec<String> = Theme::themer_dir()
            .and_then(|d| std::fs::read_dir(d).ok())
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let p = e.path();
                let name = p.file_stem()?.to_str()?.to_string();
                let src = std::fs::read_to_string(&p).ok()?;
                let doc = src.parse::<toml::Table>().ok()?;
                let dark = doc.get("appearance").and_then(|v| v.as_str()).unwrap_or("dark") == "dark";
                (p.extension()? == "toml" && dark).then_some(name)
            })
            .collect();
        themer.sort();
        v.extend(themer);
        v
    }

    /// A built-in palette by name, else a themer theme, else neon.
    pub fn load(name: Option<&str>) -> Self {
        let name = name.unwrap_or("neon");
        if let Some(t) = Theme::builtin(name) {
            return t;
        }
        Theme::load_themer(name).unwrap_or_else(|| Theme::builtin("neon").unwrap())
    }

    /// A themer theme layered over neon, so one that omits a key still renders.
    fn load_themer(name: &str) -> Option<Self> {
        let src = std::fs::read_to_string(Theme::themer_dir()?.join(format!("{name}.toml"))).ok()?;
        let doc = src.parse::<toml::Table>().ok()?;
        let mut t = Theme::builtin("neon").unwrap();
        t.name = name.to_string();
        if let Some(p) = doc.get("palette").and_then(|v| v.as_table()) {
            let get = |k: &str| p.get(k).and_then(|v| v.as_str()).and_then(hex);
            let set = |dst: &mut Rgba, k: &str| {
                if let Some(c) = get(k) {
                    *dst = c;
                }
            };
            set(&mut t.fg, "fg");
            set(&mut t.cursor, "cursor");
            set(&mut t.selection_bg, "selection_bg");
            for (i, k) in ANSI_KEYS.iter().enumerate() {
                set(&mut t.ansi[i], k);
            }
            set(&mut t.accent, "accent");
            set(&mut t.warning, "warning");
            set(&mut t.error, "error");
            set(&mut t.ok, "ok");
            // themer's `surface` is a UI-chrome gray (bars, pills); the
            // terminal panel should be the theme's terminal background, with
            // the window around it a shade deeper, as in punkterm's palettes.
            if let Some(bg) = get("bg") {
                t.surface = bg;
                t.bg = mix(bg, [0.0, 0.0, 0.0, 1.0], 0.45);
            }
            t.surface_alt = get("surface").map(|c| mix(t.surface, c, 0.6)).unwrap_or(t.surface_alt);
            set(&mut t.border, "border");
            set(&mut t.muted, "muted");
            // A second accent for gradients: the theme's cyan reads as the
            // natural partner to most accents.
            t.accent2 = get("accent2").unwrap_or(t.ansi[6]);
            t.alert = t.error;
            t.busy = t.accent2;
            t.ready = t.ok;
        }
        if let Some(f) = doc.get("fonts").and_then(|v| v.as_table()) {
            if let Some(m) = f.get("mono").and_then(|v| v.as_str()) {
                t.mono_font = m.to_string();
                t.ui_font = format!("{m} Propo");
            }
            if let Some(s) = f.get("mono_size").and_then(|v| v.as_integer().map(|i| i as f64).or(v.as_float())) {
                // themer sizes are points; render in pixels.
                t.mono_px = (s as f32 * 4.0 / 3.0).round();
            }
        }
        Some(t)
    }

    /// A light theme: its terminal background is brighter than mid-gray.
    pub fn is_light(&self) -> bool {
        let [r, g, b, _] = self.surface;
        0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5
    }

    /// Blend every colour towards `to`; names and fonts come from `to`.
    pub fn lerp(&self, to: &Theme, t: f32) -> Theme {
        let m = |a: Rgba, b: Rgba| mix(a, b, t);
        let mut ansi = to.ansi;
        for (i, c) in ansi.iter_mut().enumerate() {
            *c = m(self.ansi[i], *c);
        }
        Theme {
            name: to.name.clone(),
            bg: m(self.bg, to.bg),
            fg: m(self.fg, to.fg),
            cursor: m(self.cursor, to.cursor),
            selection_bg: m(self.selection_bg, to.selection_bg),
            ansi,
            accent: m(self.accent, to.accent),
            accent2: m(self.accent2, to.accent2),
            warning: m(self.warning, to.warning),
            error: m(self.error, to.error),
            ok: m(self.ok, to.ok),
            surface: m(self.surface, to.surface),
            surface_alt: m(self.surface_alt, to.surface_alt),
            border: m(self.border, to.border),
            muted: m(self.muted, to.muted),
            alert: m(self.alert, to.alert),
            busy: m(self.busy, to.busy),
            ready: m(self.ready, to.ready),
            mono_font: to.mono_font.clone(),
            ui_font: to.ui_font.clone(),
            mono_px: to.mono_px,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse() {
        for n in BUILTINS {
            assert!(Theme::builtin(n).is_some(), "{n}");
        }
    }

    #[test]
    fn light_and_dark() {
        assert!(Theme::builtin("burnt-orange-light").unwrap().is_light());
        assert!(!Theme::builtin("neon").unwrap().is_light());
    }
}
