//! ARM9 processor model (ARMv5TE: the ARM and Thumb instruction sets, the
//! enhanced DSP instructions, and the CP15 system control coprocessor with
//! its MMU), as in the ARM926EJ-S. Written for the LeapFrog Zippity
//! (GeneralPlus GPL32300A); see docs/zippity.md.
//!
//! Sources: the ARM Architecture Reference Manual (ARMv5TE: instruction
//! semantics, exception model, the VMSAv5 MMU) and the ARM926EJ-S Technical
//! Reference Manual (CP15 register map, ID values). Cycle timing is one
//! cycle per instruction (placeholder): nothing measured yet.

#![allow(clippy::needless_range_loop)]

mod arm;
mod cp15;
mod thumb;

pub use cp15::Cp15;

/// The memory and devices the processor sees (physical addresses).
pub trait ArmBus {
    fn read8(&mut self, addr: u32) -> u8;
    fn read16(&mut self, addr: u32) -> u16;
    fn read32(&mut self, addr: u32) -> u32;
    fn write8(&mut self, addr: u32, value: u8);
    fn write16(&mut self, addr: u32, value: u16);
    fn write32(&mut self, addr: u32, value: u32);
}

/// Processor modes (CPSR bits 4-0).
pub mod mode {
    pub const USR: u32 = 0x10;
    pub const FIQ: u32 = 0x11;
    pub const IRQ: u32 = 0x12;
    pub const SVC: u32 = 0x13;
    pub const ABT: u32 = 0x17;
    pub const UND: u32 = 0x1B;
    pub const SYS: u32 = 0x1F;
}

pub mod psr {
    pub const N: u32 = 1 << 31;
    pub const Z: u32 = 1 << 30;
    pub const C: u32 = 1 << 29;
    pub const V: u32 = 1 << 28;
    pub const Q: u32 = 1 << 27;
    pub const I: u32 = 1 << 7;
    pub const F: u32 = 1 << 6;
    pub const T: u32 = 1 << 5;
    pub const MODE: u32 = 0x1F;
}

/// Exception vectors (offsets from the vector base).
pub mod vector {
    pub const RESET: u32 = 0x00;
    pub const UNDEFINED: u32 = 0x04;
    pub const SWI: u32 = 0x08;
    pub const PREFETCH_ABORT: u32 = 0x0C;
    pub const DATA_ABORT: u32 = 0x10;
    pub const IRQ: u32 = 0x18;
    pub const FIQ: u32 = 0x1C;
}

/// Index of a mode's banked registers: User/System, SVC, ABT, UND, IRQ, FIQ.
fn bank(m: u32) -> usize {
    match m {
        mode::SVC => 1,
        mode::ABT => 2,
        mode::UND => 3,
        mode::IRQ => 4,
        mode::FIQ => 5,
        _ => 0,
    }
}

/// An MMU fault (VMSAv5 fault status codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault {
    pub status: u32,
    pub addr: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Arm {
    /// The current mode's registers; r15 holds the address of the next
    /// instruction to fetch.
    pub r: [u32; 16],
    pub cpsr: u32,
    /// r13/r14 of each bank (index `bank`), the inactive ones' r8-r12.
    banked_sp_lr: [[u32; 2]; 6],
    usr_r8_12: [u32; 5],
    fiq_r8_12: [u32; 5],
    /// SPSR of each exception mode (index `bank`; 0 unused).
    pub spsr: [u32; 6],
    pub cp15: Cp15,
    /// Interrupt inputs (level, active high).
    pub irq_line: bool,
    pub fiq_line: bool,
    /// Waiting for an interrupt (CP15 c7, c0, 4).
    pub waiting: bool,
    pub cycles: u64,
    pub instructions: u64,
    /// The address of the instruction being executed (diagnostics).
    pub current: u32,
    /// Undefined instructions met, for the machine to report.
    #[serde(skip)]
    pub undefined_log: Vec<(u32, u32)>,
}

impl Default for Arm {
    fn default() -> Self {
        Self::new()
    }
}

impl Arm {
    pub fn new() -> Self {
        let mut a = Self {
            r: [0; 16],
            cpsr: mode::SVC | psr::I | psr::F,
            banked_sp_lr: [[0; 2]; 6],
            usr_r8_12: [0; 5],
            fiq_r8_12: [0; 5],
            spsr: [0; 6],
            cp15: Cp15::default(),
            irq_line: false,
            fiq_line: false,
            waiting: false,
            cycles: 0,
            instructions: 0,
            current: 0,
            undefined_log: Vec::new(),
        };
        a.r[15] = 0;
        a
    }

    pub fn mode(&self) -> u32 {
        self.cpsr & psr::MODE
    }

    pub fn thumb(&self) -> bool {
        self.cpsr & psr::T != 0
    }

    /// The program counter as instructions read it (8 or 4 bytes ahead).
    pub fn pc(&self) -> u32 {
        self.current
    }

    /// Switches mode, swapping the banked registers.
    pub fn set_mode(&mut self, new: u32) {
        let old = self.mode();
        if old == new {
            return;
        }
        let (ob, nb) = (bank(old), bank(new));
        if ob != nb {
            self.banked_sp_lr[ob] = [self.r[13], self.r[14]];
            [self.r[13], self.r[14]] = self.banked_sp_lr[nb];
        }
        if (old == mode::FIQ) != (new == mode::FIQ) {
            let regs: [u32; 5] = self.r[8..13].try_into().unwrap();
            if old == mode::FIQ {
                self.fiq_r8_12 = regs;
                self.r[8..13].copy_from_slice(&self.usr_r8_12);
            } else {
                self.usr_r8_12 = regs;
                self.r[8..13].copy_from_slice(&self.fiq_r8_12);
            }
        }
        self.cpsr = (self.cpsr & !psr::MODE) | new;
    }

    /// Writes the CPSR (mode changes swap banks).
    pub fn set_cpsr(&mut self, value: u32) {
        self.set_mode(value & psr::MODE);
        self.cpsr = value;
    }

    /// The SPSR of the current mode (the CPSR in User/System mode, which
    /// have none: UNPREDICTABLE in the architecture).
    pub fn spsr(&self) -> u32 {
        match bank(self.mode()) {
            0 => self.cpsr,
            b => self.spsr[b],
        }
    }

    pub fn set_spsr(&mut self, value: u32) {
        let b = bank(self.mode());
        if b != 0 {
            self.spsr[b] = value;
        }
    }

    /// A User-mode register (LDM/STM with the S bit), whatever the mode.
    fn user_reg(&self, n: usize) -> u32 {
        match n {
            8..=12 if self.mode() == mode::FIQ => self.usr_r8_12[n - 8],
            13 | 14 if bank(self.mode()) != 0 => self.banked_sp_lr[0][n - 13],
            _ => self.r[n],
        }
    }

    fn set_user_reg(&mut self, n: usize, v: u32) {
        match n {
            8..=12 if self.mode() == mode::FIQ => self.usr_r8_12[n - 8] = v,
            13 | 14 if bank(self.mode()) != 0 => self.banked_sp_lr[0][n - 13] = v,
            _ => self.r[n] = v,
        }
    }

    /// Takes an exception: `ret` goes to the new mode's r14.
    pub fn exception(&mut self, offset: u32, new_mode: u32, ret: u32) {
        let old = self.cpsr;
        self.set_mode(new_mode);
        self.spsr[bank(new_mode)] = old;
        self.r[14] = ret;
        self.cpsr &= !psr::T;
        self.cpsr |= psr::I;
        if offset == vector::FIQ || offset == vector::RESET {
            self.cpsr |= psr::F;
        }
        self.r[15] = self.cp15.vector_base() + offset;
        self.waiting = false;
    }

    /// Reset: SVC mode, interrupts off, ARM state, at the vector base.
    pub fn reset(&mut self) {
        let cp15 = Cp15::default();
        *self = Self::new();
        self.cp15 = cp15;
        self.r[15] = self.cp15.vector_base();
    }

    // ---- memory, through the MMU --------------------------------------

    fn translate(&mut self, bus: &mut impl ArmBus, va: u32, write: bool) -> Result<u32, Fault> {
        self.cp15.translate(bus, va, write)
    }

    pub(crate) fn load32(&mut self, bus: &mut impl ArmBus, va: u32) -> Result<u32, Fault> {
        let pa = self.translate(bus, va & !3, false)?;
        // ARMv5: an unaligned word load rotates the aligned word.
        Ok(bus.read32(pa).rotate_right(8 * (va & 3)))
    }

    pub(crate) fn load16(&mut self, bus: &mut impl ArmBus, va: u32) -> Result<u32, Fault> {
        let pa = self.translate(bus, va & !1, false)?;
        Ok(bus.read16(pa) as u32)
    }

    pub(crate) fn load8(&mut self, bus: &mut impl ArmBus, va: u32) -> Result<u32, Fault> {
        let pa = self.translate(bus, va, false)?;
        Ok(bus.read8(pa) as u32)
    }

    pub(crate) fn store32(&mut self, bus: &mut impl ArmBus, va: u32, v: u32) -> Result<(), Fault> {
        let pa = self.translate(bus, va & !3, true)?;
        bus.write32(pa, v);
        Ok(())
    }

    pub(crate) fn store16(&mut self, bus: &mut impl ArmBus, va: u32, v: u32) -> Result<(), Fault> {
        let pa = self.translate(bus, va & !1, true)?;
        bus.write16(pa, v as u16);
        Ok(())
    }

    pub(crate) fn store8(&mut self, bus: &mut impl ArmBus, va: u32, v: u32) -> Result<(), Fault> {
        let pa = self.translate(bus, va, true)?;
        bus.write8(pa, v as u8);
        Ok(())
    }

    /// A data abort for the instruction being executed.
    pub(crate) fn data_abort(&mut self, f: Fault) {
        self.cp15.fsr = f.status;
        self.cp15.far = f.addr;
        let ret = self.current.wrapping_add(8);
        self.exception(vector::DATA_ABORT, mode::ABT, ret);
    }

    /// An undefined instruction (word or halfword, at `current`).
    pub(crate) fn undefined(&mut self, word: u32) {
        if self.undefined_log.len() < 64 {
            self.undefined_log.push((self.current, word));
        }
        let ret = self.current.wrapping_add(if self.thumb() { 2 } else { 4 });
        self.exception(vector::UNDEFINED, mode::UND, ret);
    }

    // ---- condition codes and flags ------------------------------------

    pub(crate) fn cond(&self, c: u32) -> bool {
        let f = self.cpsr;
        let (n, z, cy, v) = (f & psr::N != 0, f & psr::Z != 0, f & psr::C != 0, f & psr::V != 0);
        match c {
            0x0 => z,
            0x1 => !z,
            0x2 => cy,
            0x3 => !cy,
            0x4 => n,
            0x5 => !n,
            0x6 => v,
            0x7 => !v,
            0x8 => cy && !z,
            0x9 => !cy || z,
            0xA => n == v,
            0xB => n != v,
            0xC => !z && n == v,
            0xD => z || n != v,
            _ => true,
        }
    }

    pub(crate) fn set_nz(&mut self, v: u32) {
        self.cpsr = (self.cpsr & !(psr::N | psr::Z)) | (v & psr::N) | if v == 0 { psr::Z } else { 0 };
    }

    pub(crate) fn set_flag(&mut self, flag: u32, on: bool) {
        if on {
            self.cpsr |= flag;
        } else {
            self.cpsr &= !flag;
        }
    }

    pub(crate) fn carry(&self) -> bool {
        self.cpsr & psr::C != 0
    }

    // ---- execution ----------------------------------------------------

    /// Runs one instruction, or takes a pending interrupt. Returns false
    /// while waiting for an interrupt (no instruction ran).
    pub fn step(&mut self, bus: &mut impl ArmBus) -> bool {
        if self.fiq_line && self.cpsr & psr::F == 0 {
            let ret = self.r[15].wrapping_add(4);
            self.exception(vector::FIQ, mode::FIQ, ret);
        } else if self.irq_line && self.cpsr & psr::I == 0 {
            let ret = self.r[15].wrapping_add(4);
            self.exception(vector::IRQ, mode::IRQ, ret);
        }
        // An interrupt ends a wait even when the CPSR masks it (ARM926EJ-S
        // TRM, wait for interrupt); Linux idles with interrupts off.
        if self.irq_line || self.fiq_line {
            self.waiting = false;
        }
        if self.waiting {
            self.cycles += 1;
            return false;
        }
        let pc = self.r[15];
        self.current = pc;
        self.cycles += 1;
        self.instructions += 1;
        if self.thumb() {
            let word = match self.cp15.translate(bus, pc & !1, false) {
                Ok(pa) => bus.read16(pa),
                Err(f) => return self.prefetch_abort(f),
            };
            self.r[15] = pc.wrapping_add(2);
            thumb::execute(self, bus, word);
        } else {
            let word = match self.cp15.translate(bus, pc & !3, false) {
                Ok(pa) => bus.read32(pa),
                Err(f) => return self.prefetch_abort(f),
            };
            self.r[15] = pc.wrapping_add(4);
            arm::execute(self, bus, word);
        }
        true
    }

    fn prefetch_abort(&mut self, f: Fault) -> bool {
        self.cp15.ifsr = f.status;
        let ret = self.current.wrapping_add(4);
        self.exception(vector::PREFETCH_ABORT, mode::ABT, ret);
        true
    }

    /// Branch to `target` (bit 0 selects Thumb for interworking branches).
    pub(crate) fn branch_exchange(&mut self, target: u32) {
        if target & 1 != 0 {
            self.cpsr |= psr::T;
            self.r[15] = target & !1;
        } else {
            self.cpsr &= !psr::T;
            self.r[15] = target & !3;
        }
    }
}

#[cfg(test)]
mod tests;
