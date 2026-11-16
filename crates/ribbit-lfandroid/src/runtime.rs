//! A user-mode runner for a title's native library: Ribbit's ARM core (ARMv7 + VFP) over a flat
//! 32-bit memory, the library's imports trapped at their thunk slots and answered in Rust
//! (`imports.rs`), and a stand-in Java VM for its JNI calls (`jni.rs`). No operating system
//! runs: this is the "Enhanced (new)" path (b) of docs/leappad-academy.md.

use crate::elf::Loaded;
use crate::{DATA_BASE, GAME_BASE, THUNK_BASE};
use ribbit_arm::{Arm, ArmBus, V7};
use std::collections::BTreeMap;

const PAGE: usize = 0x1000;

/// Where `call` returns to: reaching it ends the call.
pub const RETURN: u32 = 0xEFFF_F000;
/// The main thread's stack (grows down from here).
pub const STACK_TOP: u32 = 0xE000_0000;
pub const STACK_SIZE: u32 = 0x0100_0000;
/// The heap (`malloc`).
pub const HEAP: u32 = 0x1000_0000;
pub const HEAP_END: u32 = 0x3000_0000;
/// The JNI function tables' slots (`jni.rs`): JNIEnv's, then JavaVM's.
pub const JNI_ENV_FUNCS: u32 = 0xF800_0000;
pub const JNI_VM_FUNCS: u32 = 0xF900_0000;
/// Functions Ribbit hands the game (OpenSL ES interfaces and the like): `host_fn`.
pub const HOST_FUNCS: u32 = 0xFA00_0000;

/// Flat memory, allocated by page as it is written.
pub struct Mem {
    pages: Vec<Option<Box<[u8; PAGE]>>>,
    /// Reads of memory never written, the first few addresses (diagnostics).
    pub unmapped: Vec<u32>,
}

impl Default for Mem {
    fn default() -> Self {
        Self { pages: (0..1usize << 20).map(|_| None).collect(), unmapped: Vec::new() }
    }
}

impl Mem {
    fn page(&mut self, a: u32) -> &mut [u8; PAGE] {
        self.pages[(a >> 12) as usize].get_or_insert_with(|| Box::new([0; PAGE]))
    }
    pub fn r8(&mut self, a: u32) -> u8 {
        match &self.pages[(a >> 12) as usize] {
            Some(p) => p[a as usize & (PAGE - 1)],
            None => {
                if self.unmapped.len() < 16 {
                    self.unmapped.push(a);
                }
                0
            }
        }
    }
    pub fn w8(&mut self, a: u32, v: u8) {
        self.page(a)[a as usize & (PAGE - 1)] = v;
    }
    pub fn r16(&mut self, a: u32) -> u16 {
        self.r8(a) as u16 | (self.r8(a.wrapping_add(1)) as u16) << 8
    }
    pub fn r32(&mut self, a: u32) -> u32 {
        if a & 3 == 0 && let Some(p) = &self.pages[(a >> 12) as usize] {
            let o = a as usize & (PAGE - 1);
            return u32::from_le_bytes(p[o..o + 4].try_into().unwrap());
        }
        self.r16(a) as u32 | (self.r16(a.wrapping_add(2)) as u32) << 16
    }
    pub fn w16(&mut self, a: u32, v: u16) {
        self.w8(a, v as u8);
        self.w8(a.wrapping_add(1), (v >> 8) as u8);
    }
    pub fn w32(&mut self, a: u32, v: u32) {
        if a & 3 == 0 {
            let o = a as usize & (PAGE - 1);
            self.page(a)[o..o + 4].copy_from_slice(&v.to_le_bytes());
            return;
        }
        self.w16(a, v as u16);
        self.w16(a.wrapping_add(2), (v >> 16) as u16);
    }
    pub fn write(&mut self, a: u32, data: &[u8]) {
        for (i, &b) in data.iter().enumerate() {
            self.w8(a.wrapping_add(i as u32), b);
        }
    }
    pub fn read(&mut self, a: u32, n: usize) -> Vec<u8> {
        (0..n).map(|i| self.r8(a.wrapping_add(i as u32))).collect()
    }
    /// A NUL-terminated string (at most 64 KB).
    pub fn cstr(&mut self, a: u32) -> String {
        let mut v = Vec::new();
        for i in 0..0x1_0000u32 {
            let b = self.r8(a.wrapping_add(i));
            if b == 0 {
                break;
            }
            v.push(b);
        }
        String::from_utf8_lossy(&v).into_owned()
    }
}

impl ArmBus for Mem {
    fn read8(&mut self, a: u32) -> u8 {
        self.r8(a)
    }
    fn read16(&mut self, a: u32) -> u16 {
        self.r16(a)
    }
    fn read32(&mut self, a: u32) -> u32 {
        self.r32(a)
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.w8(a, v)
    }
    fn write16(&mut self, a: u32, v: u16) {
        self.w16(a, v)
    }
    fn write32(&mut self, a: u32, v: u32) {
        self.w32(a, v)
    }
}

/// Why a run stopped short.
#[derive(Debug)]
pub enum Stop {
    /// An import Ribbit doesn't answer yet (its name, and where it was called from).
    Import(String, u32),
    /// A JNI function not answered yet (table, index, caller).
    Jni(&'static str, usize, u32),
    /// The instruction budget ran out (pc).
    Budget(u32),
    /// The game asked to stop (abort, exit, an undefined instruction...).
    Halt(String),
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Stop::Import(n, lr) => write!(f, "import {n} not answered (called from {lr:#010x})"),
            Stop::Jni(t, i, lr) => write!(f, "{t} function {i} not answered (called from {lr:#010x})"),
            Stop::Budget(pc) => write!(f, "instruction budget spent at {pc:#010x}"),
            Stop::Halt(s) => write!(f, "halted: {s}"),
        }
    }
}

/// A native the library registered with `RegisterNatives`.
#[derive(Debug, Clone)]
pub struct Native {
    pub class: String,
    pub signature: String,
    pub addr: u32,
}

pub struct Runtime {
    pub cpu: Arm,
    pub mem: Mem,
    /// Import names by thunk slot.
    pub thunks: Vec<String>,
    pub exports: BTreeMap<String, u32>,
    pub init: Vec<u32>,
    heap_next: u32,
    /// Allocation sizes by address (free, realloc).
    pub allocs: BTreeMap<u32, u32>,
    pub jni: crate::jni::Jni,
    pub natives: BTreeMap<String, Native>,
    /// OpenSL ES / OpenMAX AL state (`sles.rs`).
    pub sles: crate::sles::Sles,
    /// What the game logged (`__android_log_print`, stdio).
    pub log: Vec<String>,
    /// Instructions left before `Stop::Budget`.
    pub budget: u64,
    /// Imports called, by name (first-pass survey).
    pub calls: BTreeMap<String, u64>,
    /// Errors raised inside a nested call, carried out.
    pub(crate) pending: Option<Stop>,
    /// pthread keys' values (one thread so far).
    pub(crate) tls: BTreeMap<u32, u32>,
    pub(crate) next_key: u32,
    /// Host functions handed out (`host_fn`), by slot.
    pub(crate) host: Vec<String>,
    /// Things the import layer keeps between calls.
    pub(crate) state: crate::imports::State,
}

impl Runtime {
    /// Puts the loaded library in memory and sets up the stack, the JNI tables and the
    /// imports' data slots.
    pub fn new(lib: &Loaded) -> Self {
        let mut mem = Mem::default();
        mem.write(lib.base, &lib.image);
        let mut thunks = Vec::new();
        for (name, &a) in &lib.imports {
            if (THUNK_BASE..DATA_BASE).contains(&a) {
                let i = ((a - THUNK_BASE) / 8) as usize;
                if i >= thunks.len() {
                    thunks.resize(i + 1, String::new());
                }
                thunks[i] = name.clone();
            }
        }
        let mut cpu = Arm::new();
        cpu.v7 = Some(Box::<V7>::default());
        cpu.set_mode(ribbit_arm::mode::USR);
        cpu.r[13] = STACK_TOP;
        let mut rt = Runtime {
            cpu,
            mem,
            thunks,
            exports: lib.exports.clone(),
            init: lib.init.clone(),
            heap_next: HEAP,
            allocs: BTreeMap::new(),
            jni: Default::default(),
            natives: BTreeMap::new(),
            sles: Default::default(),
            log: Vec::new(),
            budget: 2_000_000_000,
            calls: BTreeMap::new(),
            pending: None,
            tls: BTreeMap::new(),
            next_key: 1,
            host: Vec::new(),
            state: Default::default(),
        };
        // Data imports (__stack_chk_guard, __sF, _ctype_, SL_IID_*): the code reads them
        // where they were bound.
        for (name, &a) in &lib.imports {
            if (DATA_BASE..JNI_ENV_FUNCS).contains(&a) {
                crate::imports::init_data(&mut rt, name, a);
            }
        }
        crate::jni::install(&mut rt);
        rt
    }

    pub fn malloc(&mut self, n: u32) -> u32 {
        let n = n.max(1).next_multiple_of(16);
        let a = self.heap_next;
        self.heap_next += n;
        assert!(self.heap_next < HEAP_END, "lfandroid: heap full");
        self.allocs.insert(a, n);
        a
    }

    pub fn free(&mut self, a: u32) {
        self.allocs.remove(&a);
    }

    /// A NUL-terminated copy of `s` on the heap.
    pub fn alloc_str(&mut self, s: &str) -> u32 {
        let a = self.malloc(s.len() as u32 + 1);
        self.mem.write(a, s.as_bytes());
        self.mem.w8(a + s.len() as u32, 0);
        a
    }

    /// A function the game can call that Ribbit answers (`imports::host_call`), by name.
    pub fn host_fn(&mut self, name: &str) -> u32 {
        let i = match self.host.iter().position(|h| h == name) {
            Some(i) => i,
            None => {
                self.host.push(name.to_string());
                self.host.len() - 1
            }
        };
        HOST_FUNCS + 8 * i as u32
    }

    /// The `i`th argument of the function being answered (AAPCS: r0-r3, then the stack).
    pub fn arg(&mut self, i: usize) -> u32 {
        if i < 4 { self.cpu.r[i] } else { self.mem.r32(self.cpu.r[13] + 4 * (i as u32 - 4)) }
    }

    /// Calls the game's function at `addr` (bit 0: Thumb) with `args`, until it returns.
    pub fn call(&mut self, addr: u32, args: &[u32]) -> Result<u32, Stop> {
        let saved = (self.cpu.r, self.cpu.cpsr);
        let mut sp = self.cpu.r[13];
        if args.len() > 4 {
            let extra = &args[4..];
            sp -= (4 * extra.len() as u32).next_multiple_of(8);
            for (i, &v) in extra.iter().enumerate() {
                self.mem.w32(sp + 4 * i as u32, v);
            }
        }
        sp &= !7;
        for (i, &v) in args.iter().take(4).enumerate() {
            self.cpu.r[i] = v;
        }
        self.cpu.r[13] = sp;
        self.cpu.r[14] = RETURN;
        self.jump(addr);
        let out = self.run();
        let r0 = self.cpu.r[0];
        self.cpu.r = saved.0;
        self.cpu.cpsr = saved.1;
        out.map(|()| r0)
    }

    fn jump(&mut self, addr: u32) {
        let t = ribbit_arm::psr::T;
        if addr & 1 != 0 {
            self.cpu.cpsr |= t;
        } else {
            self.cpu.cpsr &= !t;
        }
        self.cpu.r[15] = addr & !1;
    }

    /// Returns from the function being answered with `v` in r0.
    pub fn ret(&mut self, v: u32) {
        self.cpu.r[0] = v;
        let lr = self.cpu.r[14];
        self.jump(lr);
    }

    /// Returns a 64-bit value (r0, r1).
    pub fn ret64(&mut self, v: u64) {
        self.cpu.r[1] = (v >> 32) as u32;
        self.ret(v as u32);
    }

    fn run(&mut self) -> Result<(), Stop> {
        loop {
            let pc = self.cpu.r[15];
            if pc == RETURN {
                return Ok(());
            }
            if pc >= THUNK_BASE && !(DATA_BASE..JNI_ENV_FUNCS).contains(&pc) {
                let lr = self.cpu.r[14];
                let r = if pc >= HOST_FUNCS {
                    let i = ((pc - HOST_FUNCS) / 8) as usize;
                    let name = self.host.get(i).cloned().unwrap_or_default();
                    crate::imports::host_call(self, &name)
                } else if pc >= JNI_VM_FUNCS {
                    crate::jni::vm_call(self, ((pc - JNI_VM_FUNCS) / 8) as usize)
                } else if pc >= JNI_ENV_FUNCS {
                    crate::jni::env_call(self, ((pc - JNI_ENV_FUNCS) / 8) as usize)
                } else {
                    let i = ((pc - THUNK_BASE) / 8) as usize;
                    let name = self.thunks.get(i).cloned().unwrap_or_default();
                    *self.calls.entry(name.clone()).or_default() += 1;
                    crate::imports::call(self, &name)
                };
                if let Some(e) = self.pending.take() {
                    return Err(e);
                }
                r?;
                // Still at the slot: the answer forgot to return.
                if self.cpu.r[15] == pc {
                    let _ = lr;
                    return Err(Stop::Halt(format!("import at {pc:#x} didn't return")));
                }
                continue;
            }
            if self.budget == 0 {
                return Err(Stop::Budget(pc));
            }
            self.budget -= 1;
            self.cpu.step(&mut self.mem);
            if let Some(&(at, w)) = self.cpu.undefined_log.last() {
                self.cpu.undefined_log.clear();
                return Err(Stop::Halt(format!("undefined instruction {w:#010x} at {at:#010x}")));
            }
            if self.cpu.mode() != ribbit_arm::mode::USR {
                return Err(Stop::Halt(format!("exception into mode {:#x} from {:#010x}", self.cpu.mode(), self.cpu.current)));
            }
        }
    }

    /// Runs the constructors (DT_INIT, then DT_INIT_ARRAY in order), as the dynamic linker
    /// does before `JNI_OnLoad`.
    pub fn run_constructors(&mut self) -> Result<(), Stop> {
        for f in self.init.clone() {
            if f == 0 || f == u32::MAX {
                continue;
            }
            self.call(f, &[])?;
        }
        Ok(())
    }

    /// The library's `JNI_OnLoad(vm, NULL)`: it registers its natives.
    pub fn jni_onload(&mut self) -> Result<u32, Stop> {
        let f = *self.exports.get("JNI_OnLoad").ok_or_else(|| Stop::Halt("no JNI_OnLoad".into()))?;
        let vm = self.jni.vm;
        self.call(f, &[vm, 0])
    }
}

/// Where the library sits (for messages).
pub fn in_library(a: u32) -> bool {
    (GAME_BASE..GAME_BASE + 0x0100_0000).contains(&a)
}
