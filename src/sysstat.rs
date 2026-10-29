//! Live readouts for the HUD: CPU and memory from /proc, disk use of `/`
//! from `df`, and local time; and the same for a remote host over ssh.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// A host's load: CPU, memory and root filesystem use, 0 to 1 (disk
/// `None` when `df` could not say).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Load {
    pub cpu: f32,
    pub mem: f32,
    pub disk: Option<f32>,
}

pub struct SysStat {
    last: Instant,
    prev: Option<(u64, u64)>,
    pub cpu: f32,
    pub mem: f32,
    /// Use of `/`, resampled every 30 s (it changes slowly; `df` is a process).
    pub disk: Option<f32>,
    disk_at: Option<Instant>,
    utc_offset: i64,
}

impl SysStat {
    pub fn new() -> Self {
        let mut s = SysStat { last: Instant::now(), prev: None, cpu: 0.0, mem: 0.0, disk: None, disk_at: None, utc_offset: utc_offset() };
        s.sample();
        s
    }

    /// Resample at most once a second.
    pub fn tick(&mut self) {
        if self.last.elapsed().as_millis() >= 1000 {
            self.last = Instant::now();
            self.sample();
        }
        if self.disk_at.is_none_or(|t| t.elapsed().as_secs() >= 30) {
            self.disk_at = Some(Instant::now());
            let out = std::process::Command::new("df").args(["-Pk", "/"]).stderr(std::process::Stdio::null()).output().ok();
            self.disk = out.and_then(|o| parse_df(&String::from_utf8_lossy(&o.stdout)));
        }
    }

    pub fn load(&self) -> Load {
        Load { cpu: self.cpu, mem: self.mem, disk: self.disk }
    }

    fn sample(&mut self) {
        if let Ok(stat) = std::fs::read_to_string("/proc/stat") {
            if let Some(line) = stat.lines().next() {
                let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
                if v.len() >= 5 {
                    let idle = v[3] + v[4];
                    let total: u64 = v.iter().sum();
                    if let Some((pi, pt)) = self.prev {
                        let dt = total.saturating_sub(pt).max(1) as f32;
                        self.cpu = 1.0 - idle.saturating_sub(pi) as f32 / dt;
                    }
                    self.prev = Some((idle, total));
                }
            }
        }
        if let Ok(mi) = std::fs::read_to_string("/proc/meminfo") {
            let field = |k: &str| {
                mi.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f32>().ok())
            };
            if let (Some(t), Some(a)) = (field("MemTotal:"), field("MemAvailable:")) {
                self.mem = 1.0 - a / t.max(1.0);
            }
        }
    }

    /// "14:32" for a Unix-ms time today (local), "TUE 14:32" this week,
    /// "12d ago" before that.
    pub fn stamp(&self, ms: u64) -> String {
        let at = ms as i64 / 1000 + self.utc_offset;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) + self.utc_offset;
        let (days, d) = (now.div_euclid(86_400) - at.div_euclid(86_400), at.rem_euclid(86_400));
        let hm = format!("{:02}:{:02}", d / 3600, (d / 60) % 60);
        // 1970-01-01 was a Thursday.
        const DAYS: [&str; 7] = ["THU", "FRI", "SAT", "SUN", "MON", "TUE", "WED"];
        match days {
            0 => hm,
            1..7 => format!("{} {hm}", DAYS[at.div_euclid(86_400).rem_euclid(7) as usize]),
            _ => format!("{days}d ago"),
        }
    }

    pub fn clock(&self) -> String {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) + self.utc_offset;
        let d = secs.rem_euclid(86_400);
        format!("{:02}:{:02}:{:02}", d / 3600, (d / 60) % 60, d % 60)
    }
}

/// The local UTC offset in seconds, asked of `date` once at startup.
fn utc_offset() -> i64 {
    let out = std::process::Command::new("date").arg("+%z").output().ok();
    let s = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    if s.len() != 5 {
        return 0;
    }
    let sign = if s.starts_with('-') { -1 } else { 1 };
    let h: i64 = s[1..3].parse().unwrap_or(0);
    let m: i64 = s[3..5].parse().unwrap_or(0);
    sign * (h * 3600 + m * 60)
}

/// CPU and memory use of a remote host over ssh (Linux: /proc), sampled
/// over half a second. `None` when the host can't say.
pub fn remote(spec: &str) -> Option<Load> {
    let script = "head -1 /proc/stat; sleep 0.5; head -1 /proc/stat; grep -E '^(MemTotal|MemAvailable):' /proc/meminfo; df -Pk / | tail -1";
    let out = std::process::Command::new("ssh")
        .args(crate::tmuxcc::ssh_args(spec))
        .arg(script)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    parse_remote(&String::from_utf8_lossy(&out.stdout))
}

/// Use of a filesystem from `df -Pk` output: used / (used + available), as
/// df's own percentage counts it (root's reserve left out).
fn parse_df(out: &str) -> Option<f32> {
    let line = out.lines().rfind(|l| l.split_whitespace().count() >= 6 && l.split_whitespace().nth(2).is_some_and(|v| v.parse::<u64>().is_ok()))?;
    let f: Vec<u64> = line.split_whitespace().skip(2).take(2).filter_map(|v| v.parse().ok()).collect();
    let [used, avail] = f[..] else { return None };
    Some((used as f32 / (used + avail).max(1) as f32).clamp(0.0, 1.0))
}

fn parse_remote(out: &str) -> Option<Load> {
    let cpu: Vec<(u64, u64)> = out
        .lines()
        .filter(|l| l.starts_with("cpu "))
        .filter_map(|l| {
            let v: Vec<u64> = l.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
            (v.len() >= 5).then(|| (v[3] + v[4], v.iter().sum()))
        })
        .collect();
    let [(i0, t0), (i1, t1)] = cpu[..] else { return None };
    let cpu = 1.0 - i1.saturating_sub(i0) as f32 / t1.saturating_sub(t0).max(1) as f32;
    let field = |k: &str| out.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f32>().ok());
    let mem = 1.0 - field("MemAvailable:")? / field("MemTotal:")?.max(1.0);
    Some(Load { cpu: cpu.clamp(0.0, 1.0), mem: mem.clamp(0.0, 1.0), disk: parse_df(out) })
}

#[cfg(test)]
mod tests {
    #[test]
    fn remote_sample() {
        let out = "cpu  100 0 100 700 100 0 0 0 0 0\ncpu  150 0 150 750 150 0 0 0 0 0\nMemTotal:       1000 kB\nMemAvailable:    250 kB\n/dev/root        81106868 68940000 12150484      86% /\n";
        let l = super::parse_remote(out).unwrap();
        assert!((l.cpu - 0.5).abs() < 1e-6, "{}", l.cpu);
        assert!((l.mem - 0.75).abs() < 1e-6, "{}", l.mem);
        assert!((l.disk.unwrap() - 0.85).abs() < 0.01, "{:?}", l.disk);
        assert!(super::parse_remote("cpu  1 2 3 4 5\n").is_none());
    }
}
