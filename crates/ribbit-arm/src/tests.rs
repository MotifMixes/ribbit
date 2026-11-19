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

/// Interpreter speed on flat RAM (MMU off): a load/add/store/count loop.
/// `cargo test --release -p ribbit-arm bench_loop -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_loop() {
    let code = [0xE592_1000, 0xE280_0001, 0xE582_0004, 0xE253_3001, 0x1AFF_FFFA];
    let n = 100_000_000;
    let t = std::time::Instant::now();
    let (a, _) = run_arm(&code, n, |a, _| {
        a.r[2] = 0x8000;
        a.r[3] = u32::MAX;
    });
    let s = t.elapsed().as_secs_f64();
    println!("{} M instructions/s (r0 {})", (n as f64 / s / 1e6) as u64, a.r[0]);
}

// ---- ARMv7 (v7.rs) ------------------------------------------------------

fn run_v7(code: &[u32], setup: impl FnOnce(&mut Arm, &mut Ram)) -> (Arm, Ram) {
    run_arm(code, code.len(), |a, bus| {
        a.v7 = Some(Box::default());
        setup(a, bus)
    })
}

#[test]
fn v7_off_leaves_armv5_alone() {
    // Without v7 MOVW is not one (the ARM926 model treats the encoding as before).
    let (a, _) = run_arm(&[0xE301_0234], 1, |_, _| {});
    assert_eq!(a.r[0], 0);
    // VFP and the media instructions are undefined there.
    let (a, _) = run_arm(&[0xEE00_1A10], 1, |_, _| {});
    assert_eq!(a.mode(), mode::UND);
}

#[test]
fn v7_movw_movt_and_extends() {
    // movw r0,#0x1234; movt r1,#0x678 (r1 low half kept); uxtb r1,r2; rev r3? (rev r1,r2 -> r1)
    let (a, _) = run_v7(&[0xE301_0234, 0xE340_1678], |a, _| a.r[1] = 0xAAAA_5555);
    assert_eq!(a.r[0], 0x1234);
    assert_eq!(a.r[1], 0x0678_5555);
    let (a, _) = run_v7(&[0xE6EF_1072], |a, _| a.r[2] = 0x1234_56F8);
    assert_eq!(a.r[1], 0xF8);
    let (a, _) = run_v7(&[0xE6BF_1F32], |a, _| a.r[2] = 0x1122_3344);
    assert_eq!(a.r[1], 0x4433_2211);
    let (a, _) = run_v7(&[0xE6BF_1FB2], |a, _| a.r[2] = 0x1122_3344);
    assert_eq!(a.r[1], 0x2211_4433);
    let (a, _) = run_v7(&[0xE6FF_2F32], |a, _| a.r[2] = 1);
    assert_eq!(a.r[2], 0x8000_0000);
}

#[test]
fn v7_bitfields_and_mls() {
    // ubfx r1,r0,#4,#8 ; sbfx r1,r0,#4,#8
    let (a, _) = run_v7(&[0xE7E7_1250], |a, _| a.r[0] = 0x0000_0F80);
    assert_eq!(a.r[1], 0xF8);
    let (a, _) = run_v7(&[0xE7A7_1250], |a, _| a.r[0] = 0x0000_0F80);
    assert_eq!(a.r[1], 0xFFFF_FFF8);
    // bfi r0,r1,#3,#5
    let (a, _) = run_v7(&[0xE7C7_0191], |a, _| {
        a.r[0] = 0xFFFF_FFFF;
        a.r[1] = 0;
    });
    assert_eq!(a.r[0], 0xFFFF_FF07);
    // mls r3,r0,r1,r2: r3 = r2 - r0*r1
    let (a, _) = run_v7(&[0xE063_2190], |a, _| {
        a.r[0] = 3;
        a.r[1] = 4;
        a.r[2] = 100;
    });
    assert_eq!(a.r[3], 88);
}

#[test]
fn v7_exclusives() {
    // ldrex r2,[r0]; strex r3,r1,[r0]; strex r3,r1,[r0] (monitor cleared: fails)
    let (a, mut bus) = run_v7(&[0xE190_2F9F, 0xE180_3F91, 0xE180_3F91], |a, bus| {
        a.r[0] = 0x2000;
        a.r[1] = 77;
        bus.write32(0x2000, 5);
    });
    assert_eq!(a.r[2], 5);
    assert_eq!(a.r[3], 1);
    assert_eq!(bus.read32(0x2000), 77);
}

#[test]
fn v7_vfp_convert_and_arith() {
    // vmov s0,r1; vcvt.f32.s32 s0,s0; vmov s1,r1? use: s1 = 2.0 via vmov.f32 s0,#2 later.
    // vmov s0,r1 ; vcvt.f32.s32 s0,s0 ; vmov r2,s0
    let (a, _) = run_v7(&[0xEE00_1A10, 0xEEB8_0AC0, 0xEE10_2A10], |a, _| a.r[1] = -3i32 as u32);
    assert_eq!(f32::from_bits(a.r[2]), -3.0);
    // vmov.f32 s0,#2.0 ; vmov s1? -> vadd s0,s0,s1 with s1 = 0.5 ; vmul ; vcvt.s32.f32
    let (a, _) = run_v7(&[0xEEB0_0A00, 0xEE30_0A20, 0xEE20_0A20, 0xEEBD_0AC0, 0xEE10_2A10], |a, _| {
        a.v7.as_mut().unwrap().s[1] = 4.0f32.to_bits();
    });
    // (2 + 4) * 4 = 24
    assert_eq!(a.r[2], 24);
    // vcmpe.f32 s0,s1 ; vmrs APSR_nzcv,fpscr : 1 < 2 sets N.
    let (a, _) = run_v7(&[0xEEB4_0AE0, 0xEEF1_FA10], |a, _| {
        let st = a.v7.as_mut().unwrap();
        st.s[0] = 1.0f32.to_bits();
        st.s[1] = 2.0f32.to_bits();
    });
    assert!(a.cpsr & psr::N != 0 && a.cpsr & psr::Z == 0);
}

#[test]
fn v7_vfp_memory_and_doubles() {
    // vldr d0,[r0] ; vcvt.f64? (d0 holds 1.5) ; vmov r2,r3,d0 ; vpush {d8,d9} ; vpop {d8,d9} ; vstr s0,[r0,#4]
    let (a, mut bus) = run_v7(&[0xED90_0B00, 0xEC53_2B10, 0xED2D_8B04, 0xECBD_8B04, 0xED80_0A01], |a, bus| {
        a.r[0] = 0x2000;
        a.r[13] = 0x3000;
        let v = 1.5f64.to_bits();
        bus.write32(0x2000, v as u32);
        bus.write32(0x2004, (v >> 32) as u32);
        let st = a.v7.as_mut().unwrap();
        st.set_d(8, 11);
        st.set_d(9, 22);
    });
    assert_eq!(f64::from_bits(a.r[2] as u64 | (a.r[3] as u64) << 32), 1.5);
    assert_eq!(a.r[13], 0x3000);
    assert_eq!(bus.read32(0x3000 - 16), 11);
    assert_eq!(bus.read32(0x3000 - 8), 22);
    let st = a.v7.as_ref().unwrap();
    assert_eq!((st.d(8), st.d(9)), (11, 22));
    assert_eq!(bus.read32(0x2004), 1.5f64.to_bits() as u32);
}

// ---- Thumb-2 (thumb2.rs) and NEON ------------------------------------------

fn run_t2(code: &[u16], steps: usize, setup: impl FnOnce(&mut Arm, &mut Ram)) -> (Arm, Ram) {
    run_thumb(code, steps, |a, bus| {
        a.v7 = Some(Box::default());
        setup(a, bus)
    })
}

#[test]
fn t2_movw_and_modified_immediate() {
    // movw r0,#0x1234; add.w r1,r0,#0x00FF00FF
    let (a, _) = run_t2(&[0xF241, 0x2034, 0xF100, 0x11FF], 2, |_, _| {});
    assert_eq!(a.r[0], 0x1234);
    assert_eq!(a.r[1], 0x1234 + 0x00FF_00FF);
    assert_eq!(a.r[15], 0x1008);
}

#[test]
fn t2_it_block_keeps_flags() {
    // cmp r0,#0; ite eq; movs r1,#1 (then: runs, flags kept); movs r2,#2 (else: skipped)
    let (a, _) = run_t2(&[0x2800, 0xBF0C, 0x2101, 0x2202], 4, |_, _| {});
    assert_eq!((a.r[1], a.r[2]), (1, 0));
    assert!(a.cpsr & psr::Z != 0);
    assert_eq!(a.cpsr & (3 << 25 | 0x3F << 10), 0);
}

#[test]
fn t2_cbz_bl_tbb() {
    // cbz r0,+4 skips to movs r2,#5
    let (a, _) = run_t2(&[0xB110, 0x2101, 0x2102, 0x2103, 0x2205], 2, |_, _| {});
    assert_eq!((a.r[1], a.r[2]), (0, 5));
    // bl 0x1010
    let (a, _) = run_t2(&[0xF000, 0xF806], 1, |_, _| {});
    assert_eq!((a.r[15], a.r[14]), (0x1010, 0x1005));
    // tbb [pc, r0] with the table {2, 3} after it: r0 = 1 -> 0x1004 + 6
    let (a, _) = run_t2(&[0xE8DF, 0xF000, 0x0302], 1, |a, _| a.r[0] = 1);
    assert_eq!(a.r[15], 0x100A);
}

#[test]
fn t2_loads_stores_push_pop() {
    // ldr.w r2,[pc,#8]: from Align(0x1004) + 8
    let (a, _) = run_t2(&[0xF8DF, 0x2008], 1, |_, bus| bus.write32(0x100C, 0xDEAD_BEEF));
    assert_eq!(a.r[2], 0xDEAD_BEEF);
    // push.w {r4,lr}; mov r4,#0 (movs r4,#0); pop.w {r4,pc}
    let (a, bus) = run_t2(&[0xE92D, 0x4010, 0x2400, 0xE8BD, 0x8010], 3, |a, _| {
        a.r[13] = 0x8000;
        a.r[4] = 7;
        a.r[14] = 0x2001;
    });
    let mut bus = bus;
    assert_eq!((bus.read32(0x7FF8), bus.read32(0x7FFC)), (7, 0x2001));
    assert_eq!((a.r[4], a.r[13], a.r[15]), (7, 0x8000, 0x2000));
    assert!(a.thumb());
}

#[test]
fn t2_divide_and_vfp_literal() {
    // udiv r0,r1,r2
    let (a, _) = run_t2(&[0xFBB1, 0xF0F2], 1, |a, _| {
        a.r[1] = 100;
        a.r[2] = 7;
    });
    assert_eq!(a.r[0], 14);
    // vldr s0,[pc,#4]: from Align(0x1004) + 4
    let (a, _) = run_t2(&[0xED9F, 0x0A01], 1, |_, bus| bus.write32(0x1008, 0x3F80_0000));
    assert_eq!(a.v7.as_ref().unwrap().s[0], 0x3F80_0000);
}

#[test]
fn neon_vadd_vsub_i64() {
    // vadd.i64 d16,d17,d18; vsub.i64 d16,d17,d18 (U = 1)
    let (a, _) = run_v7(&[0xF271_08A2], |a, _| {
        let v = a.v7.as_mut().unwrap();
        v.set_d(17, 0xFFFF_FFFF);
        v.set_d(18, 1);
    });
    assert_eq!(a.v7.as_ref().unwrap().d(16), 0x1_0000_0000);
    let (a, _) = run_v7(&[0xF371_08A2], |a, _| {
        let v = a.v7.as_mut().unwrap();
        v.set_d(17, 0);
        v.set_d(18, 1);
    });
    assert_eq!(a.v7.as_ref().unwrap().d(16), u64::MAX);
}
