//! CP15, the system control coprocessor (ARM926EJ-S TRM chapter 2), and the
//! MMU (ARMv5 VMSA: sections, coarse and fine page tables; large, small and
//! tiny pages). Access permissions and domains are checked only to the
//! extent of "no access" domains (UNVERIFIED need: firmware so far relies on
//! translation, not protection). Caches are not modelled (the instruction
//! and data caches are coherent with memory in this model, so their
//! maintenance operations do nothing).

use crate::{ArmBus, Fault};

/// ARM926EJ-S main ID (TRM: 0x41069265, revision 5).
const MAIN_ID: u32 = 0x4106_9265;
/// Cache type: 16 KB I, 16 KB D, 4-way, 32-byte lines (a typical ARM926
/// configuration; the GPL32300A's real sizes are UNKNOWN).
const CACHE_TYPE: u32 = 0x1D15_2152;
/// TLB entries cached (direct mapped by virtual page).
const TLB: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Cp15 {
    /// c1: M (MMU) bit 0, A bit 1, C bit 2, W bit 3, S bit 8, R bit 9,
    /// I bit 12, V (high vectors) bit 13, RR bit 14, L4 bit 15.
    pub control: u32,
    pub ttb: u32,
    pub dacr: u32,
    pub fsr: u32,
    pub ifsr: u32,
    pub far: u32,
    pub fcse_pid: u32,
    pub context_id: u32,
    /// Writes to registers this model does not use, by (crn, op1, crm, op2).
    #[serde(skip)]
    pub other: std::collections::BTreeMap<(u32, u32, u32, u32), u32>,
    /// (virtual page | 1, physical page) per slot; 0 = empty.
    #[serde(skip, default = "empty_tlb")]
    tlb: Vec<(u32, u32)>,
}

fn empty_tlb() -> Vec<(u32, u32)> {
    vec![(0, 0); TLB]
}

impl Default for Cp15 {
    fn default() -> Self {
        // Reset value (TRM): 0x0005_0078 (SBO bits, little-endian, MMU off).
        Self {
            control: 0x0005_0078,
            ttb: 0,
            dacr: 0,
            fsr: 0,
            ifsr: 0,
            far: 0,
            fcse_pid: 0,
            context_id: 0,
            other: Default::default(),
            tlb: empty_tlb(),
        }
    }
}

impl Cp15 {
    pub fn mmu_on(&self) -> bool {
        self.control & 1 != 0
    }

    pub fn vector_base(&self) -> u32 {
        if self.control & (1 << 13) != 0 { 0xFFFF_0000 } else { 0 }
    }

    pub fn flush_tlb(&mut self) {
        self.tlb.iter_mut().for_each(|e| *e = (0, 0));
    }

    /// MRC p15: the register's value.
    pub fn read(&self, crn: u32, op1: u32, crm: u32, op2: u32) -> u32 {
        match (crn, op2) {
            (0, 1) => CACHE_TYPE,
            (0, 2) => 0, // TCM status: none
            (0, _) => MAIN_ID,
            (1, _) => self.control,
            (2, _) => self.ttb,
            (3, _) => self.dacr,
            (5, 1) => self.ifsr,
            (5, _) => self.fsr,
            (6, _) => self.far,
            // c7 "test and clean" (op2 3, crm 10/14): report "all clean"
            // (bit 30), which loops waiting for it expect.
            (7, 3) => 1 << 30,
            (13, 0) => self.fcse_pid,
            (13, 1) => self.context_id,
            _ => self.other.get(&(crn, op1, crm, op2)).copied().unwrap_or(0),
        }
    }

    /// MCR p15. Returns true for "wait for interrupt".
    pub fn write(&mut self, crn: u32, op1: u32, crm: u32, op2: u32, v: u32) -> bool {
        match (crn, crm, op2) {
            (1, _, _) => {
                self.control = v;
                self.flush_tlb();
            }
            (2, _, _) => {
                self.ttb = v & 0xFFFF_C000;
                self.flush_tlb();
            }
            (3, _, _) => {
                self.dacr = v;
                self.flush_tlb();
            }
            (5, _, 1) => self.ifsr = v,
            (5, _, _) => self.fsr = v,
            (6, _, _) => self.far = v,
            // Cache maintenance: nothing to do; c7 c0 4 waits for an interrupt.
            (7, 0, 4) => return true,
            (7, 8, 2) => return true,
            (7, _, _) => {}
            // TLB maintenance.
            (8, _, _) => self.flush_tlb(),
            (13, _, 0) => {
                self.fcse_pid = v & 0xFE00_0000;
                self.flush_tlb();
            }
            (13, _, 1) => self.context_id = v,
            _ => {
                self.other.insert((crn, op1, crm, op2), v);
            }
        }
        false
    }

    /// Virtual to physical (the FCSE, then the MMU when on).
    pub fn translate(&mut self, bus: &mut impl ArmBus, va: u32, _write: bool) -> Result<u32, Fault> {
        let va = if va < 0x0200_0000 { va | self.fcse_pid } else { va };
        if !self.mmu_on() {
            return Ok(va);
        }
        let slot = (va >> 12) as usize % TLB;
        let (tag, page) = self.tlb[slot];
        if tag == (va & !0xFFF) | 1 {
            return Ok(page | (va & 0xFFF));
        }
        let pa = self.walk(bus, va)?;
        self.tlb[slot] = ((va & !0xFFF) | 1, pa & !0xFFF);
        Ok(pa)
    }

    fn walk(&self, bus: &mut impl ArmBus, va: u32) -> Result<u32, Fault> {
        let l1 = bus.read32(self.ttb | ((va >> 20) << 2));
        let domain = (l1 >> 5) & 0xF;
        let domain_none = (self.dacr >> (2 * domain)) & 3 == 0;
        let fault = |status: u32| Err(Fault { status: status | (domain << 4), addr: va });
        match l1 & 3 {
            0 => fault(0x5), // section translation fault
            2 => {
                if domain_none {
                    return fault(0x9);
                }
                Ok((l1 & 0xFFF0_0000) | (va & 0x000F_FFFF))
            }
            kind => {
                let l2_addr = if kind == 1 {
                    (l1 & 0xFFFF_FC00) | (((va >> 12) & 0xFF) << 2)
                } else {
                    (l1 & 0xFFFF_F000) | (((va >> 10) & 0x3FF) << 2)
                };
                let l2 = bus.read32(l2_addr);
                if domain_none {
                    return fault(0xB);
                }
                match l2 & 3 {
                    0 => fault(0x7), // page translation fault
                    1 => Ok((l2 & 0xFFFF_0000) | (va & 0xFFFF)),
                    2 => Ok((l2 & 0xFFFF_F000) | (va & 0xFFF)),
                    _ => Ok((l2 & 0xFFFF_FC00) | (va & 0x3FF)),
                }
            }
        }
    }
}
