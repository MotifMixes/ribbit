//! Which instruction shapes busy code executes (for interpreter tuning):
//!   RIBBIT_CART=<cart> RIBBIT_STATE=<state> cargo run --release -p ribbit-core --example histogram -- <bios> [steps]

use ribbit_core::Machine;
use ribbit_cpu::isa::{Dst, Src};
use ribbit_debug::Breakpoints;
use std::collections::HashMap;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: histogram <bios> [steps]");
    let steps: u64 = args.next().map_or(20_000_000, |s| s.parse().expect("steps"));
    let fw = ribbit_memory::Firmware::from_bytes(ribbit_core::read_image(&path).unwrap()).unwrap();
    let cart = std::env::var("RIBBIT_CART")
        .ok()
        .map(|c| ribbit_memory::Cartridge::from_bytes(ribbit_core::read_image(&c).unwrap()).unwrap());
    let mut m = Machine::with_straps(fw, cart, 0x63FF_FFFF);
    if let Ok(state) = std::env::var("RIBBIT_STATE") {
        m.load_state(&std::fs::read(&state).unwrap()).unwrap();
    }
    m.idle_skip = false;
    let mut shapes: HashMap<String, u64> = HashMap::new();
    let mut ops: HashMap<String, u64> = HashMap::new();
    let kind = |s: Src| match s {
        Src::None => "-",
        Src::Reg(r) if r < 32 => "r",
        Src::Reg(_) => "R",
        Src::Imm(_) => "i",
        Src::Limm(_) => "L",
    };
    let dkind = |d: Dst| match d {
        Dst::None => "-",
        Dst::Reg(r) if r < 32 => "r",
        Dst::Reg(_) => "R",
        Dst::Discard => "x",
    };
    let (mut taken_cc, mut ccs) = (0u64, 0u64);
    let mut general_n = 0u64;
    for _ in 0..steps {
        m.cpu.last_instr = None;
        m.run(1, &Breakpoints::default());
        let Some(i) = m.cpu.last_instr else { continue };
        let op = format!("{:?}", i.op);
        let op = op.split('(').next().unwrap().to_string();
        *ops.entry(op.clone()).or_default() += 1;
        let s = format!(
            "{op:8} {}{}{} d{} cc{} f{} dl{} s{}",
            dkind(i.dst),
            kind(i.src1),
            kind(i.src2),
            kind(i.data),
            i.cc,
            i.set_flags as u8,
            i.delay as u8,
            i.short as u8
        );
        let general = ribbit_cpu::icache::fast_form(&i) == ribbit_cpu::icache::Fast::General;
        if general {
            general_n += 1;
        }
        if std::env::var_os("GENERAL_ONLY").is_none() || general {
            *shapes.entry(format!("{s} wb{:?} sz{:?}{}", i.mem.wb, i.mem.size, if i.mem.sign_extend { "x" } else { "" })).or_default() += 1;
        }
        if i.cc != 0 {
            ccs += 1;
            taken_cc += 1;
        }
    }
    let _ = taken_cc;
    let total: u64 = ops.values().sum();
    let mut v: Vec<_> = ops.into_iter().collect();
    v.sort_by_key(|r| std::cmp::Reverse(r.1));
    println!(
        "{total} instructions; conditional {:.1}%; general executor {:.1}%",
        100.0 * ccs as f64 / total as f64,
        100.0 * general_n as f64 / total as f64
    );
    for (k, n) in v.iter().take(30) {
        println!("{:6.2}% {k}", 100.0 * *n as f64 / total as f64);
    }
    let mut v: Vec<_> = shapes.into_iter().collect();
    v.sort_by_key(|r| std::cmp::Reverse(r.1));
    let mut cum = 0.0;
    for (k, n) in v.iter().take(60) {
        let p = 100.0 * *n as f64 / total as f64;
        cum += p;
        println!("{p:6.2}% {cum:6.1}% {k}");
    }
}
