//! Process memory and CPU samples.
//!
//! Canvas records the process memory before and after a request and the CPU
//! the request burned into `x-canvas-meta`
//! (`app/middleware/request_throttle.rb:308-312`):
//!
//! ```text
//! b=<memory before>;m=<memory after>;u=<user cpu>;y=<system cpu>;
//! ```
//!
//! The port reads the same facts from `/proc`, which Linux (the CI runner)
//! and macOS both provide. When `/proc` is not there the numbers are zero —
//! the header is still well formed.

/// A snapshot of the process' memory and CPU counters.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcSample {
    /// Resident set size in bytes.
    memory: u64,
    /// User CPU seconds the process has used.
    user_cpu: f64,
    /// System CPU seconds the process has used.
    system_cpu: f64,
}

impl ProcSample {
    /// Take a snapshot.
    pub fn capture() -> Self {
        let (memory, user_cpu, system_cpu) = read_proc();
        Self {
            memory,
            user_cpu,
            system_cpu,
        }
    }

    /// The resident set size in bytes.
    pub fn memory_bytes(&self) -> u64 {
        self.memory
    }

    /// The user CPU seconds this request burned.
    pub fn user_cpu_delta(&self, after: &ProcSample) -> f64 {
        (after.user_cpu - self.user_cpu).max(0.0)
    }

    /// The system CPU seconds this request burned.
    pub fn system_cpu_delta(&self, after: &ProcSample) -> f64 {
        (after.system_cpu - self.system_cpu).max(0.0)
    }
}

/// Read memory and CPU counters from `/proc`.
fn read_proc() -> (u64, f64, f64) {
    let memory = read_memory();
    let (user_cpu, system_cpu) = read_cpu();
    (memory, user_cpu, system_cpu)
}

/// The resident set size in bytes, from `/proc/self/statm`.
fn read_memory() -> u64 {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    let Some(resident_pages) = statm.split_whitespace().nth(1) else {
        return 0;
    };
    let Ok(pages) = resident_pages.parse::<u64>() else {
        return 0;
    };
    // Linux reports pages of 4 KiB.
    pages * 4096
}

/// The user and system CPU seconds, from `/proc/self/stat`.
fn read_cpu() -> (f64, f64) {
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else {
        return (0.0, 0.0);
    };
    // The command name is in parentheses and may contain spaces, so start
    // after the last ')'.
    let Some(rest) = stat.rfind(')').map(|index| &stat[index + 1..]) else {
        return (0.0, 0.0);
    };
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After the command, field 11 is utime and field 12 is stime (the first
    // field of `rest` is `state`, field 3 of /proc/self/stat).
    let Some(utime) = fields.get(11).and_then(|value| value.parse::<f64>().ok()) else {
        return (0.0, 0.0);
    };
    let Some(stime) = fields.get(12).and_then(|value| value.parse::<f64>().ok()) else {
        return (0.0, 0.0);
    };
    // The counters are in clock ticks; Linux uses 100 per second.
    (utime / 100.0, stime / 100.0)
}
