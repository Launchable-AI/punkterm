//! Live readouts for the HUD: CPU and memory from /proc, and local time.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct SysStat {
    last: Instant,
    prev: Option<(u64, u64)>,
    pub cpu: f32,
    pub mem: f32,
    utc_offset: i64,
}

impl SysStat {
    pub fn new() -> Self {
        let mut s = SysStat { last: Instant::now(), prev: None, cpu: 0.0, mem: 0.0, utc_offset: utc_offset() };
        s.sample();
        s
    }

    /// Resample at most once a second.
    pub fn tick(&mut self) {
        if self.last.elapsed().as_millis() >= 1000 {
            self.last = Instant::now();
            self.sample();
        }
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
