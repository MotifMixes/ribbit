//! Instruction mix of a stretch of emulated time, for building a timing
//! model: where code is fetched from and data accessed, taken branches,
//! loads whose result the next instruction uses, long immediates,
//! multiplies, LCD transfers, and the miss rates of the caches the BIOS's
//! cache-locking routines describe (US v2.1 0x4003_8DF8 / 0x4003_8EC8:
//! 16-byte lines, set index from address bits 4-12, way in bits 13-14, so
//! 4 ways x 512 sets = 32 KB each for instructions and data), simulated
//! here with LRU replacement (the real policy is UNKNOWN).
//!   cargo run --release -p ribbit-core --example mix -- <bios> <cart> <state> <ms> [system.eeprom] [cart.sav]
//! <state> is a save state of the same BIOS and cartridge.
use ribbit_core::Machine;
use ribbit_cpu::isa::{Dst, Op, Size, Src, WriteBack};
use ribbit_debug::Breakpoints;
use std::collections::BTreeMap;

fn region(a: u32) -> &'static str {
    match a >> 24 {
        0x3C => "ram",
        0x03 => "vram",
        0x40 => "bios",
        0x80..=0x8F => "cart",
        0x01 => "mmio",
        0x00 => "low",
        _ => "other",
    }
}

/// A 4-way, 512-set, 16-byte-line cache with LRU replacement.
struct Cache {
    tags: Vec<[u32; 4]>,
    age: Vec<[u64; 4]>,
    clock: u64,
    hits: u64,
    misses: u64,
}

impl Cache {
    fn new() -> Self {
        Self { tags: vec![[u32::MAX; 4]; 512], age: vec![[0; 4]; 512], clock: 0, hits: 0, misses: 0 }
    }

    /// Touches the line holding `addr`; true on a hit.
    fn access(&mut self, addr: u32) -> bool {
        self.clock += 1;
        let (line, set) = (addr >> 4, (addr >> 4) as usize & 511);
        if let Some(w) = self.tags[set].iter().position(|&t| t == line) {
            self.age[set][w] = self.clock;
            self.hits += 1;
            return true;
        }
        let w = (0..4).min_by_key(|&w| self.age[set][w]).unwrap();
        self.tags[set][w] = line;
        self.age[set][w] = self.clock;
        self.misses += 1;
        false
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [bios, cart, state, ms, rest @ ..] = &a[..] else {
        panic!("usage: mix <bios> <cart> <state> <ms> [system.eeprom] [cart.sav]");
    };
    let fw = ribbit_memory::Firmware::from_bytes(ribbit_core::read_image(bios).unwrap()).unwrap();
    let cart = ribbit_memory::Cartridge::from_bytes(ribbit_core::read_image(cart).unwrap()).unwrap();
    let mut m = Machine::with_straps(fw, Some(cart), 0x63FF_FFFF);
    if let Some(p) = rest.first() {
        m.io.eeprom.load(&std::fs::read(p).unwrap());
    }
    if let Some(p) = rest.get(1) {
        m.io.eeprom.insert_cart(Some(&std::fs::read(p).unwrap()));
    }
    m.enable_experimental_mame_touch(Default::default());
    m.record_trace_tail = false;
    m.load_state(&std::fs::read(state).unwrap()).unwrap();
    m.idle_skip = false;
    // (`run` keeps `last_instr` only while recording the trace tail.)
    m.record_trace_tail = true;
    let none = Breakpoints::default();
    let per_ms = m.clock.cpu_hz / 1000;
    let end = m.cpu.cycles + ms.parse::<u64>().unwrap() * per_ms;
    let lcd0 = m.io.lcd.transfers;
    let clk0 = (m.io.clock_control.value, m.io.clock_control.writes);

    let mut steps = 0u64;
    let mut fetch: BTreeMap<&str, (u64, u64)> = BTreeMap::new(); // (instructions, halfwords)
    let mut data: BTreeMap<(&str, &str), u64> = BTreeMap::new(); // (region, ld/st) -> count
    let mut ops: BTreeMap<String, u64> = BTreeMap::new();
    let (mut taken, mut taken_nd, mut brcc_taken, mut load_use, mut limm, mut mul, mut halted) = (0u64, 0, 0, 0, 0, 0, 0);
    let mut last_load: Option<u8> = None;
    let (mut icache, mut dcache) = (Cache::new(), Cache::new());
    let mut imiss: BTreeMap<&str, u64> = BTreeMap::new();
    let mut dmiss: BTreeMap<(&str, &str), u64> = BTreeMap::new();
    let mut bypass = 0u64;
    while m.cpu.cycles < end {
        let regs = m.cpu.regs;
        let pc = m.cpu.pc;
        m.run(1, &none);
        let Some(i) = m.cpu.last_instr.take() else {
            halted += 1;
            continue;
        };
        if i.addr != pc {
            // An interrupt entry or other non-instruction step.
            halted += 1;
            continue;
        }
        steps += 1;
        let f = fetch.entry(region(i.addr)).or_default();
        f.0 += 1;
        f.1 += (i.len / 2) as u64;
        // Every line the instruction (with its long immediate) occupies.
        for line in (i.addr >> 4)..=((i.addr + i.len - 1) >> 4) {
            if !icache.access(line << 4) {
                *imiss.entry(region(i.addr)).or_default() += 1;
            }
        }
        if i.limm.is_some() {
            limm += 1;
        }
        let reads = |s: Src, r: u8| matches!(s, Src::Reg(x) if x == r);
        if let Some(r) = last_load
            && (reads(i.src1, r) || reads(i.src2, r) || reads(i.data, r))
        {
            load_use += 1;
        }
        last_load = None;
        let val = |s: Src| match s {
            Src::Reg(r) => regs[r as usize],
            Src::Imm(v) | Src::Limm(v) => v,
            Src::None => 0,
        };
        let ea = |sp_base: bool| {
            let base = if sp_base { regs[28] } else { val(i.src1) };
            let off = val(i.src2);
            let scale = match i.mem.size {
                Size::Word => 2,
                Size::Half => 1,
                Size::Byte => 0,
            };
            match i.mem.wb {
                WriteBack::Post => base,
                WriteBack::Scaled => base.wrapping_add(off << scale),
                _ if i.op == Op::Push => base.wrapping_sub(4),
                _ => base.wrapping_add(off),
            }
        };
        let mut dtouch = |addr: u32, kind: &'static str, di: bool| {
            if di || region(addr) == "mmio" {
                bypass += 1;
            } else if !dcache.access(addr) {
                *dmiss.entry((region(addr), kind)).or_default() += 1;
            }
        };
        if matches!(i.op, Op::Ld | Op::Pop) {
            dtouch(ea(i.op == Op::Pop), "ld", i.mem.bypass_cache);
        } else if matches!(i.op, Op::St | Op::Push) {
            dtouch(ea(i.op == Op::Push), "st", i.mem.bypass_cache);
        }
        let name = match i.op {
            Op::Ld | Op::Pop => {
                let base = if i.op == Op::Pop { regs[28] } else { val(i.src1) };
                *data.entry((region(base), "ld")).or_default() += 1;
                if let Dst::Reg(r) = i.dst {
                    last_load = Some(r);
                }
                "ld"
            }
            Op::St | Op::Push => {
                let base = if i.op == Op::Push { regs[28] } else { val(i.src1) };
                *data.entry((region(base), "st")).or_default() += 1;
                "st"
            }
            Op::Mul64 | Op::Mulu64 => {
                mul += 1;
                "mul"
            }
            Op::B | Op::Bl | Op::J | Op::Jl | Op::Br(_) | Op::Bbit0 | Op::Bbit1 => "branch",
            Op::Lp => "lp",
            Op::Lr | Op::Sr => "aux",
            _ => "alu",
        };
        *ops.entry(name.into()).or_default() += 1;
        if i.is_branch() && m.cpu.pc != i.next() {
            // Taken: control left the straight line (a delay slot, when
            // executed, runs first; this counts the branch as taken all the same).
            taken += 1;
            if !i.delay {
                taken_nd += 1;
            }
            if matches!(i.op, Op::Br(_) | Op::Bbit0 | Op::Bbit1) {
                brcc_taken += 1;
            }
        }
    }
    let secs = ms.parse::<f64>().unwrap() / 1000.0;
    println!("clock register 0x0180_9008: {:#x} ({} writes before), now {:#x} ({} writes in the window)", clk0.0, clk0.1, m.io.clock_control.value, m.io.clock_control.writes - clk0.1);
    let lcd = m.io.lcd.transfers - lcd0;
    println!("{steps} instructions in {secs} s ({:.1} M/s), {halted} other steps; LCD {lcd} frames ({:.1}/s)", steps as f64 / secs / 1e6, lcd as f64 / secs);
    let pct = |n: u64| 100.0 * n as f64 / steps as f64;
    println!("fetch by region (instructions, halfwords):");
    for (r, (n, h)) in &fetch {
        println!("  {r:6} {n:>11} {:5.1}%  {h:>11} halfwords", pct(*n));
    }
    println!("ops:");
    for (o, n) in &ops {
        println!("  {o:7} {n:>11} {:5.1}%", pct(*n));
    }
    println!("data accesses by region:");
    for ((r, k), n) in &data {
        println!("  {r:6} {k} {n:>11} {:5.1}%", pct(*n));
    }
    println!(
        "taken branches {taken} ({:.1}%), without .d {taken_nd} ({:.1}%), BRcc/BBIT taken {brcc_taken} ({:.1}%)",
        pct(taken),
        pct(taken_nd),
        pct(brcc_taken)
    );
    println!("load-use (next instruction reads the loaded register) {load_use} ({:.1}%)", pct(load_use));
    println!("long immediates {limm} ({:.1}%), multiplies {mul} ({:.1}%)", pct(limm), pct(mul));
    println!(
        "I-cache: {} hits, {} misses ({:.3}% of line accesses; {:.2} per 1000 instructions)",
        icache.hits,
        icache.misses,
        100.0 * icache.misses as f64 / (icache.hits + icache.misses) as f64,
        1000.0 * icache.misses as f64 / steps as f64
    );
    for (r, n) in &imiss {
        println!("  miss from {r:6} {n:>10}");
    }
    println!(
        "D-cache: {} hits, {} misses ({:.3}%); {bypass} uncached (.di or device)",
        dcache.hits,
        dcache.misses,
        100.0 * dcache.misses as f64 / (dcache.hits + dcache.misses) as f64
    );
    for ((r, k), n) in &dmiss {
        println!("  miss {k} {r:6} {n:>10}");
    }
}
