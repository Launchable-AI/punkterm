//! Optional sound: short tones synthesized at startup (no assets), written
//! to punkterm's own cache dir and played with PipeWire/Pulse's player.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// Only what needs you: blocked, and turn complete.
    Alerts,
    /// Alerts plus quiet UI ticks.
    All,
}

impl Mode {
    pub fn next(self) -> Mode {
        match self {
            Mode::Off => Mode::Alerts,
            Mode::Alerts => Mode::All,
            Mode::All => Mode::Off,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Alerts => "alerts",
            Mode::All => "all",
        }
    }
    pub fn parse(s: &str) -> Mode {
        match s {
            "alerts" => Mode::Alerts,
            "all" => Mode::All,
            _ => Mode::Off,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sfx {
    Blocked,
    Ready,
    Tick,
    Open,
    Theme,
    Blip,
}

impl Sfx {
    const ALL: [Sfx; 6] = [Sfx::Blocked, Sfx::Ready, Sfx::Tick, Sfx::Open, Sfx::Theme, Sfx::Blip];

    fn alert(self) -> bool {
        matches!(self, Sfx::Blocked | Sfx::Ready)
    }

    fn file(self) -> &'static str {
        match self {
            Sfx::Blocked => "blocked.wav",
            Sfx::Ready => "ready.wav",
            Sfx::Tick => "tick.wav",
            Sfx::Open => "open.wav",
            Sfx::Theme => "theme.wav",
            Sfx::Blip => "blip.wav",
        }
    }
}

const RATE: u32 = 48_000;

/// One voice: frequency sweep `f0`→`f1`, starting at `at` seconds, `len` long.
struct Voice {
    at: f32,
    len: f32,
    f0: f32,
    f1: f32,
    gain: f32,
}

fn render(voices: &[Voice]) -> Vec<i16> {
    let end = voices.iter().map(|v| v.at + v.len).fold(0.0, f32::max) + 0.02;
    let n = (end * RATE as f32) as usize;
    let mut buf = vec![0.0f32; n];
    for v in voices {
        let start = (v.at * RATE as f32) as usize;
        let len = (v.len * RATE as f32) as usize;
        let mut phase = 0.0f32;
        for i in 0..len {
            let t = i as f32 / RATE as f32;
            let p = i as f32 / len as f32;
            let f = v.f0 + (v.f1 - v.f0) * p;
            phase += std::f32::consts::TAU * f / RATE as f32;
            // Fast attack, exponential tail: a struck, glassy tone.
            let env = (t / 0.004).min(1.0) * (-p * 5.0).exp() * (1.0 - p).max(0.0);
            let s = phase.sin() + 0.18 * (2.0 * phase).sin() + 0.06 * (3.0 * phase).sin();
            if let Some(o) = buf.get_mut(start + i) {
                *o += s * env * v.gain;
            }
        }
    }
    buf.iter().map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).collect()
}

fn synth(s: Sfx) -> Vec<i16> {
    let v = |at, len, f0, f1, gain| Voice { at, len, f0, f1, gain };
    match s {
        // Two low-high pulses, repeated: unmistakable without being shrill.
        Sfx::Blocked => render(&[
            v(0.00, 0.11, 587.0, 587.0, 0.30),
            v(0.09, 0.14, 880.0, 880.0, 0.26),
            v(0.30, 0.11, 587.0, 587.0, 0.22),
            v(0.39, 0.16, 880.0, 880.0, 0.20),
        ]),
        // A rising fifth: done, your move.
        Sfx::Ready => render(&[v(0.00, 0.16, 784.0, 784.0, 0.22), v(0.08, 0.30, 1175.0, 1175.0, 0.20)]),
        Sfx::Tick => render(&[v(0.0, 0.025, 2400.0, 2200.0, 0.10)]),
        Sfx::Open => render(&[v(0.0, 0.14, 520.0, 1040.0, 0.14)]),
        Sfx::Theme => render(&[
            v(0.00, 0.10, 659.0, 659.0, 0.12),
            v(0.05, 0.10, 880.0, 880.0, 0.12),
            v(0.10, 0.18, 1319.0, 1319.0, 0.12),
        ]),
        Sfx::Blip => render(&[v(0.0, 0.05, 1500.0, 1300.0, 0.08)]),
    }
}

fn wav(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut o = Vec::with_capacity(44 + data_len as usize);
    o.extend_from_slice(b"RIFF");
    o.extend_from_slice(&(36 + data_len).to_le_bytes());
    o.extend_from_slice(b"WAVEfmt ");
    o.extend_from_slice(&16u32.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes()); // PCM
    o.extend_from_slice(&1u16.to_le_bytes()); // mono
    o.extend_from_slice(&RATE.to_le_bytes());
    o.extend_from_slice(&(RATE * 2).to_le_bytes());
    o.extend_from_slice(&2u16.to_le_bytes());
    o.extend_from_slice(&16u16.to_le_bytes());
    o.extend_from_slice(b"data");
    o.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        o.extend_from_slice(&s.to_le_bytes());
    }
    o
}

pub struct Sound {
    pub mode: Mode,
    dir: Option<PathBuf>,
    player: Option<&'static str>,
    last: HashMap<Sfx, Instant>,
}

impl Sound {
    pub fn new(mode: Mode) -> Self {
        let player = ["pw-play", "paplay"].into_iter().find(|p| {
            Command::new(p).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
        });
        let mut s = Sound { mode, dir: None, player, last: HashMap::new() };
        if mode != Mode::Off {
            s.prepare();
        }
        s
    }

    pub fn available(&self) -> bool {
        self.player.is_some()
    }

    fn prepare(&mut self) {
        if self.dir.is_some() {
            return;
        }
        let Some(dir) = dirs::cache_dir().map(|d| d.join("punkterm").join("sfx")) else { return };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        for s in Sfx::ALL {
            let _ = std::fs::write(dir.join(s.file()), wav(&synth(s)));
        }
        self.dir = Some(dir);
    }

    pub fn set_mode(&mut self, m: Mode) {
        self.mode = m;
        if m != Mode::Off {
            self.prepare();
        }
    }

    pub fn play(&mut self, s: Sfx) {
        let on = match self.mode {
            Mode::Off => false,
            Mode::Alerts => s.alert(),
            Mode::All => true,
        };
        if !on {
            return;
        }
        let (Some(dir), Some(player)) = (&self.dir, self.player) else { return };
        // Several sessions finishing at once should not stack into noise.
        if self.last.get(&s).is_some_and(|t| t.elapsed().as_millis() < 250) {
            return;
        }
        self.last.insert(s, Instant::now());
        let path = dir.join(s.file());
        std::thread::spawn(move || {
            let _ = Command::new(player).arg(path).stdout(Stdio::null()).stderr(Stdio::null()).status();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_well_formed() {
        let w = wav(&synth(Sfx::Ready));
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()) as usize, w.len() - 8);
    }
}
