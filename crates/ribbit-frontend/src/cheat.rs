//! Cheat / memory search: scan guest RAM for values, filter across
//! multiple passes, and optionally freeze (continually rewrite) them.

use ribbit_core::Machine;

/// How to match candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchOp {
    /// Exact match to a typed value.
    #[default]
    Equal,
    /// Not equal to a typed value.
    NotEqual,
    /// Less than a typed value.
    Less,
    /// Greater than a typed value.
    Greater,
    /// Decreased from the previous scan (no typed value needed).
    Decreased,
    /// Increased from the previous scan (no typed value needed).
    Increased,
    /// Unchanged from the previous scan.
    Unchanged,
}

impl SearchOp {
    pub fn needs_value(self) -> bool {
        matches!(self, SearchOp::Equal | SearchOp::NotEqual | SearchOp::Less | SearchOp::Greater)
    }

    pub fn label(self) -> &'static str {
        match self {
            SearchOp::Equal => "=",
            SearchOp::NotEqual => "≠",
            SearchOp::Less => "<",
            SearchOp::Greater => ">",
            SearchOp::Decreased => "decreased",
            SearchOp::Increased => "increased",
            SearchOp::Unchanged => "unchanged",
        }
    }
}

/// The width and signedness of each candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ValueType {
    #[default]
    U8,
    U16,
    U32,
}

impl ValueType {
    pub fn byte_count(self) -> u32 {
        match self {
            ValueType::U8 => 1,
            ValueType::U16 => 2,
            ValueType::U32 => 4,
        }
    }

    /// The largest value that fits.
    pub fn max(self) -> u32 {
        match self {
            ValueType::U8 => u8::MAX as u32,
            ValueType::U16 => u16::MAX as u32,
            ValueType::U32 => u32::MAX,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ValueType::U8 => "1 byte",
            ValueType::U16 => "2 bytes",
            ValueType::U32 => "4 bytes",
        }
    }
}

/// One surviving candidate address + its value from the last scan.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub addr: u32,
    /// The value at this address as of the last scan (raw, little-endian).
    pub prev: u32,
}

/// A frozen address: the cheat engine writes `value` to `addr` every frame.
#[derive(Debug, Clone)]
pub struct Freeze {
    pub addr: u32,
    pub value: u32,
    pub ty: ValueType,
    pub label: String,
}

#[derive(Default)]
pub struct CheatSearch {
    pub op: SearchOp,
    pub ty: ValueType,
    /// Typed search value (decimal or hex with 0x prefix).
    pub value_text: String,
    pub candidates: Vec<Candidate>,
    pub freezes: Vec<Freeze>,
    /// Whether a first scan has been done (so "next scan" makes sense).
    pub has_scan: bool,
}

impl CheatSearch {
    /// Read `ty`-sized little-endian value from machine at `addr`.
    fn read(m: &Machine, addr: u32, ty: ValueType) -> Option<u32> {
        match ty {
            ValueType::U8 => m.bus.peek8(addr).map(|b| b as u32),
            ValueType::U16 => {
                let lo = m.bus.peek8(addr)? as u32;
                let hi = m.bus.peek8(addr.wrapping_add(1))? as u32;
                Some(lo | (hi << 8))
            }
            ValueType::U32 => {
                let b0 = m.bus.peek8(addr)? as u32;
                let b1 = m.bus.peek8(addr.wrapping_add(1))? as u32;
                let b2 = m.bus.peek8(addr.wrapping_add(2))? as u32;
                let b3 = m.bus.peek8(addr.wrapping_add(3))? as u32;
                Some(b0 | (b1 << 8) | (b2 << 16) | (b3 << 24))
            }
        }
    }

    /// Write `ty`-sized little-endian value to machine at `addr`.
    fn write(m: &mut Machine, addr: u32, value: u32, ty: ValueType) {
        match ty {
            ValueType::U8 => { m.bus.poke8(addr, value as u8); }
            ValueType::U16 => {
                m.bus.poke8(addr, value as u8);
                m.bus.poke8(addr.wrapping_add(1), (value >> 8) as u8);
            }
            ValueType::U32 => {
                m.bus.poke8(addr, value as u8);
                m.bus.poke8(addr.wrapping_add(1), (value >> 8) as u8);
                m.bus.poke8(addr.wrapping_add(2), (value >> 16) as u8);
                m.bus.poke8(addr.wrapping_add(3), (value >> 24) as u8);
            }
        }
    }

    /// Parse `value_text` as a u32 (decimal or `0x`-prefixed hex). None if unparseable.
    fn parse_value(&self) -> Option<u32> {
        parse_number(&self.value_text)
    }

    /// First scan: search all of RAM for addresses matching `op` / `value`.
    pub fn first_scan(&mut self, m: &Machine) {
        let search_val = if self.op.needs_value() {
            self.parse_value().unwrap_or(0)
        } else {
            0
        };
        let ram = m.bus.ram();
        let ram_base = ribbit_memory::map::RAM_BASE;
        let stride = self.ty.byte_count();
        let mut cands = Vec::new();
        let len = ram.len() as u32;
        let mut offset = 0u32;
        while offset + stride <= len {
            let addr = ram_base + offset;
            if let Some(val) = Self::read(m, addr, self.ty) {
                let keep = match self.op {
                    SearchOp::Equal => val == search_val,
                    SearchOp::NotEqual => val != search_val,
                    SearchOp::Less => val < search_val,
                    SearchOp::Greater => val > search_val,
                    // For a first scan with no previous value, accept all.
                    SearchOp::Decreased | SearchOp::Increased | SearchOp::Unchanged => true,
                };
                if keep {
                    cands.push(Candidate { addr, prev: val });
                }
            }
            offset += stride;
        }
        self.candidates = cands;
        self.has_scan = true;
    }

    /// Subsequent scan: narrow the candidates to those that still match.
    pub fn next_scan(&mut self, m: &Machine) {
        let search_val = if self.op.needs_value() {
            self.parse_value().unwrap_or(0)
        } else {
            0
        };
        self.candidates.retain_mut(|c| {
            let Some(val) = Self::read(m, c.addr, self.ty) else { return false };
            let keep = match self.op {
                SearchOp::Equal => val == search_val,
                SearchOp::NotEqual => val != search_val,
                SearchOp::Less => val < search_val,
                SearchOp::Greater => val > search_val,
                SearchOp::Decreased => val < c.prev,
                SearchOp::Increased => val > c.prev,
                SearchOp::Unchanged => val == c.prev,
            };
            c.prev = val;
            keep
        });
    }

    /// Update all candidate `prev` values from current machine state.
    #[allow(dead_code)]
    pub fn refresh_candidates(&mut self, m: &Machine) {
        for c in &mut self.candidates {
            if let Some(v) = Self::read(m, c.addr, self.ty) {
                c.prev = v;
            }
        }
    }

    /// Freeze `addr` at `value` (an address already frozen takes the new
    /// value).
    pub fn add_freeze(&mut self, addr: u32, value: u32, label: impl Into<String>) {
        let ty = self.ty;
        match self.freezes.iter_mut().find(|f| f.addr == addr) {
            Some(f) => {
                f.value = value;
                f.ty = ty;
            }
            None => self.freezes.push(Freeze { addr, value, ty, label: label.into() }),
        }
    }

    /// The value at `addr` now, at the search's size.
    pub fn current(&self, m: &Machine, addr: u32) -> Option<u32> {
        Self::read(m, addr, self.ty)
    }

    /// Writes `value` to `addr` once (the game carries on from it).
    pub fn set_once(&self, m: &mut Machine, addr: u32, value: u32) {
        Self::write(m, addr, value, self.ty);
    }

    /// Remove the freeze at `addr`.
    pub fn remove_freeze(&mut self, addr: u32) {
        self.freezes.retain(|f| f.addr != addr);
    }

    /// Apply all active freezes to the machine (call each UI frame).
    pub fn apply_freezes(&self, m: &mut Machine) {
        for f in &self.freezes {
            Self::write(m, f.addr, f.value, f.ty);
        }
    }

    /// Reset the search (clears candidates; keeps freezes).
    pub fn reset(&mut self) {
        self.candidates.clear();
        self.has_scan = false;
    }
}

/// A number as typed: decimal (negative too) or `0x`-prefixed hex.
pub fn parse_number(s: &str) -> Option<u32> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u32>().ok().or_else(|| s.parse::<i32>().ok().map(|v| v as u32))
    }
}

#[cfg(test)]
mod set_tests {
    use super::*;

    #[test]
    fn numbers_are_read_as_typed() {
        assert_eq!(parse_number("99"), Some(99));
        assert_eq!(parse_number(" 0x63 "), Some(0x63));
        assert_eq!(parse_number("-1"), Some(u32::MAX));
        assert_eq!(parse_number("lots"), None);
        assert_eq!(ValueType::U8.max(), 255);
        assert_eq!(ValueType::U16.max(), 65535);
    }

    /// Set and Freeze on a real game:
    /// `RIBBIT_BIOS=<BIOS> RIBBIT_CART=<cartridge> cargo test -p ribbit-frontend --release -- --ignored set_and_freeze`.
    /// Bytes the game leaves alone keep a Set value; a byte the game keeps
    /// changing stays at a frozen value.
    #[test]
    #[ignore]
    fn set_and_freeze_work_on_a_running_game() {
        use ribbit_debug::Breakpoints;
        let bios = std::env::var("RIBBIT_BIOS").expect("RIBBIT_BIOS");
        let cart = std::env::var("RIBBIT_CART").expect("RIBBIT_CART");
        let fw = ribbit_core::load_firmware(std::path::Path::new(&bios)).expect("BIOS");
        let cart = ribbit_core::load_cartridge(std::path::Path::new(&cart)).expect("cartridge");
        let mut m = Machine::new(fw, Some(cart));
        m.reset();
        let second = m.clock.cpu_hz as u64;
        let run = |m: &mut Machine, secs: f64| m.run_cycles((second as f64 * secs) as u64, &Breakpoints::default());
        run(&mut m, 8.0);
        // Bytes that hold still for two seconds, and bytes that keep changing.
        let mut still = CheatSearch { op: SearchOp::Unchanged, ..Default::default() };
        still.first_scan(&m);
        let mut moving = CheatSearch { op: SearchOp::Unchanged, ..Default::default() };
        moving.first_scan(&m);
        for _ in 0..4 {
            run(&mut m, 0.5);
            still.next_scan(&m);
        }
        let before: Vec<(u32, u32)> = moving.candidates.iter().map(|c| (c.addr, c.prev)).collect();
        run(&mut m, 0.5);
        let changing: Vec<u32> = before.iter().filter(|(a, v)| moving.current(&m, *a) != Some(*v)).map(|(a, _)| *a).collect();
        eprintln!("{} bytes held still, {} changed", still.candidates.len(), changing.len());
        // Set: 50 still bytes take new values, and keep them.
        let picked: Vec<(u32, u32)> = still.candidates.iter().step_by((still.candidates.len() / 50).max(1)).take(50).map(|c| (c.addr, c.prev ^ 0x5A)).collect();
        for &(a, v) in &picked {
            still.set_once(&mut m, a, v);
        }
        for &(a, v) in &picked {
            assert_eq!(still.current(&m, a), Some(v), "Set wrote {a:#x}");
        }
        run(&mut m, 2.0);
        let kept = picked.iter().filter(|&&(a, v)| still.current(&m, a) == Some(v)).count();
        eprintln!("Set: {kept} of {} kept their new value after 2 s", picked.len());
        assert!(kept * 10 >= picked.len() * 8, "Set values kept: {kept} of {}", picked.len());
        // Freeze: a byte the game changes stays at the frozen value.
        let a = *changing.first().expect("a byte the game changes");
        let mut f = CheatSearch::default();
        f.add_freeze(a, 0x42, "test");
        let mut held = 0;
        for _ in 0..60 {
            run(&mut m, 1.0 / 60.0);
            f.apply_freezes(&mut m);
            held += usize::from(f.current(&m, a) == Some(0x42));
        }
        assert_eq!(held, 60, "frozen {a:#x} held every frame");
    }

    #[test]
    fn freezing_a_frozen_address_takes_the_new_value() {
        let mut c = CheatSearch::default();
        c.add_freeze(0x1000, 3, "a");
        c.add_freeze(0x1000, 99, "a");
        assert_eq!(c.freezes.len(), 1);
        assert_eq!(c.freezes[0].value, 99);
    }
}
