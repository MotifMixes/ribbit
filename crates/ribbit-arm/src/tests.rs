use super::*;

/// 1 MB of flat RAM at address 0.
struct Ram(Vec<u8>);

impl Ram {
    fn new() -> Self {
        Self(vec![0; 0x10_0000])
    }
    fn at(&self, a: u32) -> usize {
        a as usize & 0xF_FFFF
    }
}

impl ArmBus for Ram {
    fn read8(&mut self, a: u32) -> u8 {
        self.0[self.at(a)]
    }
    fn read16(&mut self, a: u32) -> u16 {
        let i = self.at(a);
        u16::from_le_bytes([self.0[i], self.0[i + 1]])
    }
    fn read32(&mut self, a: u32) -> u32 {
        let i = self.at(a);
        u32::from_le_bytes(self.0[i..i + 4].try_into().unwrap())
    }
    fn write8(&mut self, a: u32, v: u8) {
        let i = self.at(a);
        self.0[i] = v;
    }
    fn write16(&mut self, a: u32, v: u16) {
        let i = self.at(a);
        self.0[i..i + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn write32(&mut self, a: u32, v: u32) {
        let i = self.at(a);
        self.0[i..i + 4].copy_from_slice(&v.to_le_bytes());
    }
}

/// Runs ARM words placed at 0x1000 for `steps` instructions.
fn run_arm(code: &[u32], steps: usize, setup: impl FnOnce(&mut Arm, &mut Ram)) -> (Arm, Ram) {
    let mut bus = Ram::new();
    for (i, w) in code.iter().enumerate() {
        bus.write32(0x1000 + 4 * i as u32, *w);
    }
    let mut a = Arm::new();
    a.r[15] = 0x1000;
    setup(&mut a, &mut bus);
    for _ in 0..steps {
        a.step(&mut bus);
    }
    (a, bus)
}

fn run_thumb(code: &[u16], steps: usize, setup: impl FnOnce(&mut Arm, &mut Ram)) -> (Arm, Ram) {
    let mut bus = Ram::new();
    for (i, h) in code.iter().enumerate() {
        bus.write16(0x1000 + 2 * i as u32, *h);
    }
    let mut a = Arm::new();
    a.r[15] = 0x1000;
    a.cpsr |= psr::T;
    setup(&mut a, &mut bus);
    for _ in 0..steps {
        a.step(&mut bus);
    }
    (a, bus)
}

#[test]
fn mov_add_flags() {
    // mov r0,#0xFFFFFFFF (mvn r0,#0); adds r1,r0,#1; movs r2,r0,lsr #31
    let (a, _) = run_arm(&[0xE3E0_0000, 0xE290_1001, 0xE1B0_2FA0], 3, |_, _| {});
    assert_eq!(a.r[0], 0xFFFF_FFFF);
    assert_eq!(a.r[1], 0);
    assert_eq!(a.r[2], 1);
    // movs r2,r0,lsr#31: C = bit 30 = 1, Z clear.
    assert!(a.cpsr & psr::C != 0);
    assert!(a.cpsr & psr::Z == 0);
}

#[test]
fn subs_overflow() {
    // mov r0,#0x80000000; subs r1,r0,#1  -> 0x7FFFFFFF, V set, C set
    let (a, _) = run_arm(&[0xE3A0_0102, 0xE250_1001], 2, |_, _| {});
    assert_eq!(a.r[1], 0x7FFF_FFFF);
    assert!(a.cpsr & psr::V != 0);
    assert!(a.cpsr & psr::C != 0);
}

#[test]
fn pc_reads_eight_ahead() {
    // mov r0, pc at 0x1000 -> 0x1008
    let (a, _) = run_arm(&[0xE1A0_000F], 1, |_, _| {});
    assert_eq!(a.r[0], 0x1008);
}

#[test]
fn branch_and_link() {
    // bl +8 (to 0x1010); at 0x1010: mov r0,#5
    let (a, _) = run_arm(&[0xEB00_0002, 0, 0, 0, 0xE3A0_0005], 2, |_, _| {});
    assert_eq!(a.r[14], 0x1004);
    assert_eq!(a.r[0], 5);
}

#[test]
fn ldm_stm() {
    // stmdb sp!,{r0-r2,lr}; ldmia sp!,{r4-r7}
    let (a, _) = run_arm(&[0xE92D_4007, 0xE8BD_00F0], 2, |a, _| {
        a.r[13] = 0x8000;
        a.r[0] = 1;
        a.r[1] = 2;
        a.r[2] = 3;
        a.r[14] = 4;
    });
    assert_eq!(&a.r[4..8], &[1, 2, 3, 4]);
    assert_eq!(a.r[13], 0x8000);
}

#[test]
fn unaligned_ldr_rotates() {
    // ldr r0,[r1]
    let (a, _) = run_arm(&[0xE591_0000], 1, |a, bus| {
        bus.write32(0x2000, 0x4433_2211);
        a.r[1] = 0x2001;
    });
    assert_eq!(a.r[0], 0x1144_3322);
}

#[test]
fn halfword_signed_loads() {
    // ldrsh r0,[r1]; ldrsb r2,[r1,#1]; ldrh r3,[r1]
    let (a, _) = run_arm(&[0xE1D1_00F0, 0xE1D1_20D1, 0xE1D1_30B0], 3, |a, bus| {
        bus.write16(0x2000, 0x8001);
        a.r[1] = 0x2000;
    });
    assert_eq!(a.r[0], 0xFFFF_8001);
    assert_eq!(a.r[2], 0xFFFF_FF80);
    assert_eq!(a.r[3], 0x8001);
}

#[test]
fn long_multiply_and_clz() {
    // umull r0,r1,r2,r3; smull r4,r5,r2,r3; clz r6,r2
    let (a, _) = run_arm(&[0xE081_0392, 0xE0C5_4392, 0xE16F_6F12], 3, |a, _| {
        a.r[2] = 0xFFFF_FFFF;
        a.r[3] = 2;
    });
    assert_eq!((a.r[0], a.r[1]), (0xFFFF_FFFE, 1));
    assert_eq!((a.r[4], a.r[5]), (0xFFFF_FFFE, 0xFFFF_FFFF));
    assert_eq!(a.r[6], 0);
}

#[test]
fn qadd_saturates() {
    // qadd r0,r1,r2
    let (a, _) = run_arm(&[0xE102_0051], 1, |a, _| {
        a.r[1] = 0x7FFF_FFFF;
        a.r[2] = 1;
    });
    assert_eq!(a.r[0], 0x7FFF_FFFF);
    assert!(a.cpsr & psr::Q != 0);
}

#[test]
fn smulbb_smlatt() {
    // smulbb r0,r1,r2; smlatt r3,r1,r2,r4
    let (a, _) = run_arm(&[0xE160_0281, 0xE103_42E1], 2, |a, _| {
        a.r[1] = 0x0003_FFFE; // top 3, bottom -2
        a.r[2] = 0x0005_0007; // top 5, bottom 7
        a.r[4] = 100;
    });
    assert_eq!(a.r[0] as i32, -14);
    assert_eq!(a.r[3], 115);
}

#[test]
fn swi_and_return() {
    // swi 0 at 0x1000; vector 8: movs pc, lr
    let (a, _) = run_arm(&[0xEF00_0000], 2, |a, bus| {
        bus.write32(0x8, 0xE1B0_F00E);
        a.set_cpsr(mode::USR);
        a.r[13] = 0x1234;
    });
    assert_eq!(a.r[15], 0x1004);
    assert_eq!(a.mode(), mode::USR);
    assert_eq!(a.r[13], 0x1234);
}

#[test]
fn irq_banks_and_returns() {
    let mut bus = Ram::new();
    // 0x1000: mov r0,#1; mov r0,#2   IRQ vector 0x18: subs pc,lr,#4
    bus.write32(0x1000, 0xE3A0_0001);
    bus.write32(0x1004, 0xE3A0_0002);
    bus.write32(0x18, 0xE25E_F004);
    let mut a = Arm::new();
    a.set_cpsr(mode::SYS);
    a.r[15] = 0x1000;
    a.r[13] = 0x5555;
    a.step(&mut bus);
    a.irq_line = true;
    a.step(&mut bus); // takes the IRQ, runs subs pc,lr,#4
    assert_eq!(a.mode(), mode::SYS);
    assert_eq!(a.r[15], 0x1004);
    assert_eq!(a.r[13], 0x5555);
    a.irq_line = false;
    a.step(&mut bus);
    assert_eq!(a.r[0], 2);
}

#[test]
fn bx_to_thumb_and_back() {
    let mut bus = Ram::new();
    // 0x1000: add r0,pc,#1 (0x1009); bx r0; 0x1008 thumb: movs r1,#7; bx lr (r14=0x2000)
    bus.write32(0x1000, 0xE28F_0001);
    bus.write32(0x1004, 0xE12F_FF10);
    bus.write16(0x1008, 0x2107);
    bus.write16(0x100A, 0x4770);
    let mut a = Arm::new();
    a.r[15] = 0x1000;
    a.r[14] = 0x2000;
    for _ in 0..4 {
        a.step(&mut bus);
    }
    assert_eq!(a.r[1], 7);
    assert!(!a.thumb());
    assert_eq!(a.r[15], 0x2000);
}

#[test]
fn thumb_bl_and_push_pop() {
    // 0x1000: push {r4,lr}; bl 0x100C; pop {r4,pc}  ... 0x100C: movs r4,#9; bx lr
    let code = [0xB510, 0xF000, 0xF803, 0xBD10, 0, 0, 0x2409, 0x4770];
    let (a, _) = run_thumb(&code, 6, |a, _| {
        a.r[13] = 0x8000;
        a.r[14] = 0x3001; // return to thumb 0x3000
        a.r[4] = 1;
    });
    assert_eq!(a.r[4], 1); // restored by pop
    assert_eq!(a.r[15], 0x3000);
    assert!(a.thumb());
    assert_eq!(a.r[13], 0x8000);
}

#[test]
fn thumb_ldr_pc_relative_and_alu() {
    // 0x1000: ldr r0,[pc,#4] (from 0x1008); 0x1002: negs r1,r0; 0x1004: lsls r2,r0,#4
    let code = [0x4801, 0x4241, 0x0102, 0, 0x5678, 0x1234];
    let (a, _) = run_thumb(&code, 3, |_, _| {});
    assert_eq!(a.r[0], 0x1234_5678);
    assert_eq!(a.r[1], 0x1234_5678u32.wrapping_neg());
    assert_eq!(a.r[2], 0x2345_6780);
}

#[test]
fn mmu_section_mapping() {
    let mut bus = Ram::new();
    // Translation table at 0x4000: VA 0xC0000000 -> PA 0x00000000 (section,
    // domain 0); VA 0 identity.
    bus.write32(0x4000 + (0xC00 << 2), 0x0000_0C02);
    bus.write32(0x4000, 0x0000_0C02);
    bus.write32(0x2000, 0xCAFE_F00D);
    // ldr r0,[r1]
    bus.write32(0x1000, 0xE591_0000);
    let mut a = Arm::new();
    a.cp15.ttb = 0x4000;
    a.cp15.dacr = 1;
    a.cp15.control |= 1;
    a.r[15] = 0x1000;
    a.r[1] = 0xC000_2000;
    a.step(&mut bus);
    assert_eq!(a.r[0], 0xCAFE_F00D);
}

#[test]
fn mmu_fault_aborts() {
    let mut bus = Ram::new();
    bus.write32(0x4000, 0x0000_0C02);
    bus.write32(0x1000, 0xE591_0000); // ldr r0,[r1]
    let mut a = Arm::new();
    a.cp15.ttb = 0x4000;
    a.cp15.dacr = 1;
    a.cp15.control |= 1;
    a.r[15] = 0x1000;
    a.r[1] = 0x8000_0000;
    a.step(&mut bus);
    assert_eq!(a.mode(), mode::ABT);
    assert_eq!(a.r[15], vector::DATA_ABORT);
    assert_eq!(a.r[14], 0x1008);
    assert_eq!(a.cp15.far, 0x8000_0000);
    assert_eq!(a.cp15.fsr & 0xF, 5);
}

/// A small page at VA 0x00100000 (PA 0x20000) with AP `ap` (all four subpages), domain 0 as
/// a client; VA 0 is an identity section. Runs `word` at 0x1000 in `m`.
fn page_access(ap: u32, m: u32, word: u32) -> Arm {
    let mut bus = Ram::new();
    bus.write32(0x4000, 0x0000_0C02);
    // L1 entry 1: a coarse table at 0x8000.
    bus.write32(0x4004, 0x0000_8001);
    bus.write32(0x8000, 0x0002_0002 | ap * 0x550);
    bus.write32(0x1000, word);
    let mut a = Arm::new();
    a.cp15.ttb = 0x4000;
    a.cp15.dacr = 1;
    a.cp15.control |= 1;
    a.set_cpsr(m);
    a.r[15] = 0x1000;
    a.r[1] = 0x0010_0000;
    a.step(&mut bus);
    a
}

#[test]
fn mmu_page_permissions() {
    const STR: u32 = 0xE581_0000; // str r0,[r1]
    const STRT: u32 = 0xE4A1_0000; // strt r0,[r1],#0
    // User read-only (AP 2): User writes fault (page permission, 0xF);
    // privileged writes pass, privileged STRT faults.
    let a = page_access(2, mode::SVC, STR);
    assert_eq!(a.mode(), mode::SVC);
    let a = page_access(2, mode::SVC, STRT);
    assert_eq!(a.mode(), mode::ABT);
    assert_eq!(a.cp15.fsr, 0xF);
    // The identity section at VA 0 holds the code, AP 3 there.
    let a = page_access(2, mode::USR, STR);
    assert_eq!(a.mode(), mode::ABT);
    assert_eq!(a.cp15.far, 0x0010_0000);
    // AP 3: User writes pass.
    let a = page_access(3, mode::USR, STR);
    assert_eq!(a.mode(), mode::USR);
}

#[test]
fn alignment_fault_when_a_bit_set() {
    let mut bus = Ram::new();
    bus.write32(0x1000, 0xE591_0000); // ldr r0,[r1]
    let mut a = Arm::new();
    a.cp15.control |= 2;
    a.r[15] = 0x1000;
    a.r[1] = 0x2001;
    a.step(&mut bus);
    assert_eq!(a.mode(), mode::ABT);
    assert_eq!((a.cp15.fsr, a.cp15.far), (1, 0x2001));
}
