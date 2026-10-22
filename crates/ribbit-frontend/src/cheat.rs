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
        let s = self.value_text.trim();
        if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            u32::from_str_radix(hex, 16).ok()
        } else {
            s.parse::<u32>().ok().or_else(|| s.parse::<i32>().ok().map(|v| v as u32))
        }
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

    /// Freeze `addr` at `value`.
    pub fn add_freeze(&mut self, addr: u32, value: u32, label: impl Into<String>) {
        let ty = self.ty;
        if !self.freezes.iter().any(|f| f.addr == addr) {
            self.freezes.push(Freeze { addr, value, ty, label: label.into() });
        }
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
