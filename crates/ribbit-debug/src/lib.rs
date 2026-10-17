//! Debugging infrastructure shared by all frontends.
//!
//! Log targets used across the workspace:
//! `ribbit::bus` (unmapped accesses), `ribbit::cpu`, `ribbit::irq`,
//! `ribbit::mmio`, `ribbit::boot` (milestones). Filter with `RUST_LOG`,
//! e.g. `RUST_LOG=ribbit::boot=info,ribbit::bus=trace`.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use tracing_subscriber::EnvFilter;

/// Installs a global subscriber writing to stderr (stdout stays free for
/// machine-readable output). `default_filter` applies when `RUST_LOG`
/// is unset. Safe to call more than once (later calls are ignored).
pub fn init_logging(default_filter: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_target(true).with_writer(std::io::stderr).try_init();
}

/// As `init_logging`, also writing to the file `log` (replaced each start),
/// for a windowed app whose stderr nobody sees. Without the file (not
/// writable), stderr only.
pub fn init_logging_to_file(default_filter: &str, log: &std::path::Path) {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    let Some(file) = log.parent().and_then(|d| std::fs::create_dir_all(d).ok()).and_then(|()| std::fs::File::create(log).ok())
    else {
        return init_logging(default_filter);
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let writer = std::io::stderr.and(std::sync::Mutex::new(file));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_target(true).with_ansi(false).with_writer(writer).try_init();
}

/// Classic 16-bytes-per-line hex dump with addresses.
pub fn hexdump(base: u32, data: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in data.chunks(16).enumerate() {
        let _ = write!(out, "{:08x}  ", base.wrapping_add(16 * i as u32));
        for j in 0..16 {
            match chunk.get(j) {
                Some(b) => {
                    let _ = write!(out, "{b:02x} ");
                }
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        out.extend(chunk.iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' }));
        out.push('\n');
    }
    out
}

#[derive(Debug, Default, Clone)]
pub struct Breakpoints {
    pc: BTreeSet<u32>,
}

impl Breakpoints {
    pub fn add(&mut self, pc: u32) {
        self.pc.insert(pc);
    }

    pub fn remove(&mut self, pc: u32) -> bool {
        self.pc.remove(&pc)
    }

    pub fn is_empty(&self) -> bool {
        self.pc.is_empty()
    }

    pub fn hit(&self, pc: u32) -> bool {
        !self.pc.is_empty() && self.pc.contains(&pc)
    }

    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.pc.iter().copied()
    }
}

/// Keeps the most recent `capacity` events (e.g. executed PCs) so a crash
/// can be explained by the instructions leading up to it.
#[derive(Debug, Clone)]
pub struct TraceRing<T> {
    buf: Vec<T>,
    next: usize,
    capacity: usize,
}

impl<T: Clone> TraceRing<T> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0);
        Self { buf: Vec::with_capacity(capacity), next: 0, capacity }
    }

    pub fn push(&mut self, item: T) {
        if self.buf.len() < self.capacity {
            self.buf.push(item);
        } else {
            self.buf[self.next] = item;
        }
        // Hot path (every CPU step): avoid a division.
        self.next += 1;
        if self.next == self.capacity {
            self.next = 0;
        }
    }

    /// Oldest first.
    pub fn to_vec(&self) -> Vec<T> {
        if self.buf.len() < self.capacity {
            return self.buf.clone();
        }
        let (newer, older) = self.buf.split_at(self.next);
        older.iter().chain(newer).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hexdump_formats_partial_lines() {
        let dump = hexdump(0x8000_0000, b"LEAP\x00\x01");
        assert!(dump.starts_with("80000000  4c 45 41 50 00 01 "));
        assert!(dump.ends_with("  LEAP..\n"));
        // address(10) + 16 hex cells(48) + mid gap(1) + gap(1) + ascii(6) + '\n'
        assert_eq!(dump.len(), 10 + 48 + 1 + 1 + 6 + 1);
    }

    #[test]
    fn breakpoints() {
        let mut bp = Breakpoints::default();
        assert!(!bp.hit(0x4000_0000));
        bp.add(0x4000_0000);
        assert!(bp.hit(0x4000_0000));
        assert!(bp.remove(0x4000_0000));
        assert!(!bp.hit(0x4000_0000));
    }

    #[test]
    fn trace_ring_keeps_latest_in_order() {
        let mut ring = TraceRing::new(3);
        for i in 0..5 {
            ring.push(i);
        }
        assert_eq!(ring.to_vec(), vec![2, 3, 4]);
    }
}
