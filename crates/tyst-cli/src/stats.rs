//! Process CPU time and memory, plus small latency summaries.

use std::time::Duration;

/// User + system CPU time of this process.
pub fn cpu_time() -> Duration {
    #[cfg(unix)]
    {
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: getrusage only writes into the struct we pass.
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } == 0 {
            let tv = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
            return tv(ru.ru_utime) + tv(ru.ru_stime);
        }
    }
    Duration::ZERO
}

/// Peak resident set size in MB.
pub fn peak_rss_mb() -> f64 {
    #[cfg(unix)]
    {
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: as above.
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } == 0 {
            let raw = ru.ru_maxrss as f64;
            // macOS reports bytes, Linux KiB.
            return if cfg!(target_os = "macos") { raw / (1024.0 * 1024.0) } else { raw / 1024.0 };
        }
    }
    0.0
}

/// Current resident set size in MB (Linux: /proc; elsewhere the peak is the best we have).
pub fn current_rss_mb() -> f64 {
    if let Ok(statm) = std::fs::read_to_string("/proc/self/statm")
        && let Some(pages) = statm.split_whitespace().nth(1).and_then(|p| p.parse::<f64>().ok())
    {
        return pages * 4096.0 / (1024.0 * 1024.0);
    }
    peak_rss_mb()
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Summary {
    pub count: usize,
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
}

/// Summary of values in seconds.
pub fn summarize(values: &[f64]) -> Summary {
    if values.is_empty() {
        return Summary::default();
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let pct = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    Summary {
        count: v.len(),
        mean: v.iter().sum::<f64>() / v.len() as f64,
        p50: pct(0.5),
        p95: pct(0.95),
        max: *v.last().unwrap(),
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.count == 0 {
            return write!(f, "–");
        }
        write!(f, "p50 {:.2} s · p95 {:.2} s · max {:.2} s (n = {})", self.p50, self.p95, self.max, self.count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes() {
        let s = summarize(&[0.1, 0.2, 0.3, 0.4, 1.0]);
        assert_eq!(s.count, 5);
        assert_eq!(s.p50, 0.3);
        assert_eq!(s.max, 1.0);
        assert!(cpu_time() > Duration::ZERO);
    }
}
