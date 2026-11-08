// SPDX-License-Identifier: MIT
//! Peak memory of one measured call
//!
//! The recognizer may run in this process (whisper.cpp) or in a child (transcribe-cli), and
//! the video memory belongs to either one. The probe therefore samples the process tree and
//! asks `nvidia-smi` for the compute applications inside it. A machine without the tool
//! reports no VRAM, never a zero that pretends to be a measurement

use std::collections::HashSet;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// Peak memory seen while a call ran
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryPeak {
    /// Resident set of the process tree, in bytes
    pub rss_bytes: u64,
    /// Video memory of the process tree, in bytes; `None` when no tool answered
    pub vram_bytes: Option<u64>,
}

/// A sampler for one call: start it, run the call, stop it
#[derive(Debug)]
pub struct MemoryProbe {
    stop: Arc<AtomicBool>,
    rss: Arc<AtomicU64>,
    vram: Arc<AtomicU64>,
    vram_known: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl MemoryProbe {
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let rss = Arc::new(AtomicU64::new(0));
        let vram = Arc::new(AtomicU64::new(0));
        let vram_known = Arc::new(AtomicBool::new(false));
        let stop_worker = Arc::clone(&stop);
        let rss_worker = Arc::clone(&rss);
        let vram_worker = Arc::clone(&vram);
        let known_worker = Arc::clone(&vram_known);
        let root = std::process::id();
        let worker = std::thread::spawn(move || {
            while !stop_worker.load(Ordering::Relaxed) {
                sample(root, &rss_worker, &vram_worker, &known_worker);
                std::thread::sleep(Duration::from_millis(50));
            }
            // The stop request takes a sample too, so a short call that ended between two
            // ticks still contributes its peak
            sample(root, &rss_worker, &vram_worker, &known_worker);
        });
        Self {
            stop,
            rss,
            vram,
            vram_known,
            worker: Some(worker),
        }
    }

    pub fn stop(mut self) -> MemoryPeak {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        MemoryPeak {
            rss_bytes: self.rss.load(Ordering::Relaxed),
            vram_bytes: self
                .vram_known
                .load(Ordering::Relaxed)
                .then(|| self.vram.load(Ordering::Relaxed)),
        }
    }
}

fn sample(root: u32, rss: &AtomicU64, vram: &AtomicU64, known: &AtomicBool) {
    let pids = descendants(root);
    let mut total = 0u64;
    for pid in &pids {
        total = total.saturating_add(process_rss(*pid));
    }
    rss.fetch_max(total, Ordering::Relaxed);

    if let Some(used) = process_vram(&pids) {
        known.store(true, Ordering::Relaxed);
        vram.fetch_max(used, Ordering::Relaxed);
    }
}

/// The process and every descendant that currently lives under it
fn descendants(root: u32) -> HashSet<u32> {
    let mut found = HashSet::from([root]);
    loop {
        let mut added = false;
        let Ok(entries) = std::fs::read_dir("/proc") else {
            break;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if found.contains(&pid) {
                continue;
            }
            if let Some(parent) = parent_pid(pid) {
                if found.contains(&parent) {
                    found.insert(pid);
                    added = true;
                }
            }
        }
        if !added {
            break;
        }
    }
    found
}

fn parent_pid(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:"))
        .and_then(|value| value.trim().parse().ok())
}

fn process_rss(pid: u32) -> u64 {
    let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
        return 0;
    };
    for line in status.lines() {
        // VmHWM is the high-water mark of the whole process, not the current resident size
        if let Some(value) = line.strip_prefix("VmHWM:") {
            if let Some(kib) = value
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u64>().ok())
            {
                return kib * 1024;
            }
        }
    }
    0
}

fn process_vram(pids: &HashSet<u32>) -> Option<u64> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-compute-apps=pid,used_memory",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut total = 0u64;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split(',').map(str::trim);
        let Some(pid) = parts.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(mib) = parts.next().and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        if pids.contains(&pid) {
            total = total.saturating_add(mib.saturating_mul(1024 * 1024));
        }
    }
    // A tool that answered and found no application of this tree has measured zero, not
    // missed the question
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_probe_reports_the_current_process() {
        let peak = MemoryProbe::start().stop();
        assert!(peak.rss_bytes > 0);
    }

    #[test]
    fn a_process_tree_contains_the_root() {
        let pids = descendants(std::process::id());
        assert!(pids.contains(&std::process::id()));
    }
}
