//! Execution tests. Programs come from tests/vectors/exec.s, assembled and
//! linked by the GNU ARC toolchain (scripts/gen-cpu-vectors.py), so the
//! encodings are independent of Ribbit's decoder. Expected values are worked
//! out by hand from the PRM's instruction descriptions (cited per test).

use ribbit_cpu::isa::Op;
use ribbit_cpu::{Cpu, CpuConfig, CpuError, HaltReason, IllegalPolicy, MmidMode, Step, reg, status32};
use ribbit_memory::{Firmware, SystemBus};

const BIN: &[u8] = include_bytes!("vectors/exec.bin");
const SYM: &str = include_str!("vectors/exec.sym");
const SP: u32 = 0x3C10_0000;

fn sym(name: &str) -> u32 {
    SYM.lines()
        .find_map(|l| {
            let (addr, n) = l.split_once(' ')?;
            (n == name).then(|| u32::from_str_radix(addr, 16).unwrap())
        })
        .unwrap_or_else(|| panic!("no symbol {name}"))
}

fn image_bus(image: &[u8]) -> SystemBus {
    let mut img = image.to_vec();
    img.resize(img.len().div_ceil(4) * 4, 0);
    SystemBus::new(&Firmware::from_bytes(img).unwrap(), None)
}

struct Rig {
    cpu: Cpu,
    bus: SystemBus,
}

impl Rig {
    fn new(config: CpuConfig) -> Self {
        let mut cpu = Cpu::new(config);
        cpu.regs[reg::SP] = SP;
        Rig { cpu, bus: image_bus(BIN) }
    }

    fn at(label: &str) -> Self {
        let mut r = Rig::new(CpuConfig::leapster());
        r.cpu.pc = sym(label);
        r
    }

    /// Steps until the CPU halts; panics on any CPU error.
    fn run(&mut self) -> HaltReason {
        for _ in 0..100_000 {
            match self.cpu.step(&mut self.bus) {
                Ok(Step::Halted(reason)) => return reason,
                Ok(_) => {}
                Err(e) => panic!("{e}\n{:?}", self.cpu),
            }
        }
        panic!("did not halt\n{:?}", self.cpu);
    }

    /// Steps until the CPU reports it is asleep.
    fn run_until_sleep(&mut self) {
        for _ in 0..100_000 {
            if let Step::Sleeping = self.cpu.step(&mut self.bus).unwrap() {
                return;
            }
        }
        panic!("never slept");
    }

    /// Host-style restart after BRK_S: clear H and step past the BRK_S.
    fn resume_after_brk(&mut self) {
        self.cpu.status32 &= !status32::H;
        self.cpu.pc += 2;
    }

    fn r(&self, n: usize) -> u32 {
        self.cpu.regs[n]
    }

    /// (Z, N, C, V)
    fn znvc(&self) -> (bool, bool, bool, bool) {
        let f = |b| self.cpu.status32 & b != 0;
        (f(status32::Z), f(status32::N), f(status32::C), f(status32::V))
    }
}

fn run(label: &str) -> Rig {
    let mut r = Rig::at(label);
    assert_eq!(r.run(), HaltReason::Brk);
    r
}

// ---- arithmetic and flags (PRM: ADD, ADC, SUB, SBC, CMP, condition codes) ----

#[test]
fn add_signed_overflow() {
    let r = run("t_add_overflow");
    assert_eq!(r.r(2), 0x8000_0000);
    assert_eq!(r.znvc(), (false, true, false, true));
}

#[test]
fn add_unsigned_carry_and_zero() {
    let r = run("t_add_carry");
    assert_eq!(r.r(2), 0);
    assert_eq!(r.znvc(), (true, false, true, false));
}

#[test]
fn adc_adds_carry_in() {
    assert_eq!(run("t_adc").r(4), 16);
}

#[test]
fn sub_sets_carry_as_borrow() {
    let r = run("t_sub_borrow");
    assert_eq!(r.r(2), 0xFFFF_FFFE);
    assert_eq!(r.znvc(), (false, true, true, false));
}

#[test]
fn sbc_subtracts_borrow_in() {
    let r = run("t_sbc");
    assert_eq!(r.r(4), 5);
    assert_eq!(r.znvc(), (false, false, false, false));
}

#[test]
fn sub_signed_overflow() {
    let r = run("t_sub_overflow");
    assert_eq!(r.r(2), 0x7FFF_FFFF);
    assert_eq!(r.znvc(), (false, false, false, true));
}

#[test]
fn compare_signed_and_unsigned_conditions() {
    // cmp -1,1: -1 < 1 signed (LT), 0xffffffff > 1 unsigned (HI, no carry).
    let r = run("t_cmp_conds");
    assert_eq!((r.r(10), r.r(11), r.r(12), r.r(13), r.r(14)), (1, 1, 0, 0, 1));
}

#[test]
fn scaled_add_sub_and_rsub() {
    let r = run("t_scaled_add");
    assert_eq!((r.r(3), r.r(4), r.r(5)), (106, 112, 124));
    assert_eq!((r.r(6), r.r(7), r.r(8)), (94, 88, 76));
    assert_eq!(r.r(9), 97); // rsub r9,r2,r1 = r1 - r2
}

// ---- logic and bit operations ----------------------------------------------

#[test]
fn logic_and_single_bit_operations() {
    let r = run("t_logic");
    assert_eq!(r.r(2), 0xF0);
    assert_eq!(r.r(3), 0xF0FF);
    assert_eq!(r.r(4), 0xF000);
    assert_eq!(r.r(5), 0x0F0F);
    assert_eq!(r.r(6), 0xF0F1);
    assert_eq!(r.r(7), 0xF0E0);
    assert_eq!(r.r(8), 0x70F0);
    assert_eq!(r.r(9), 0xF0);
    assert_eq!(r.r(10), 0xF0F0, "BMSK by 31 keeps all bits");
    assert_eq!(r.r(11), 0xFFFF_0F0F);
}

#[test]
fn btst_and_tst_always_set_flags() {
    let r = run("t_btst_tst");
    assert_eq!((r.r(2), r.r(3), r.r(4)), (0, 1, 1));
}

// ---- shifts (PRM: ASL, ASR, LSR, ROR, RRC, RLC, multiple shifts) -----------

#[test]
fn asl_single_sets_carry_and_overflow() {
    let r = run("t_asl_single");
    assert_eq!(r.r(2), 2);
    assert_eq!(r.znvc(), (false, false, true, true), "C = old bit 31, V = sign changed");
}

#[test]
fn single_bit_shifts_and_rotate() {
    let r = run("t_shift_single");
    assert_eq!((r.r(2), r.r(3), r.r(4)), (0xC000_0000, 0x4000_0000, 0xC000_0000));
}

#[test]
fn rotate_through_carry() {
    let r = run("t_rrc_rlc");
    assert_eq!(r.r(2), 0x8000_0001, "carry in to bit 31");
    assert_eq!(r.r(3), 4, "carry (now clear) into bit 0");
    assert!(!r.znvc().2);
}

#[test]
fn barrel_shifts_use_low_five_bits() {
    let r = run("t_multi_shift");
    assert_eq!(r.r(2), 0x810);
    assert_eq!(r.r(4), 0xF800_0000);
    assert_eq!(r.r(5), 0x40);
    assert_eq!(r.r(6), 0x1000_0008);
    assert_eq!(r.r(7), 0x8000_0000, "shift by 63 is shift by 31");
    assert!(r.znvc().2, "LSR.F by 1 shifts out bit 0");
}

#[test]
fn extend_abs_neg() {
    let r = run("t_extend");
    assert_eq!((r.r(2), r.r(3), r.r(4), r.r(5)), (0xFFFF_FF80, 0xFFFF_FF80, 0x80, 0xFF80));
    assert_eq!((r.r(7), r.r(8), r.r(13)), (5, 5, 5));
}

#[test]
fn max_min_and_their_flags() {
    let r = run("t_maxmin");
    assert_eq!((r.r(3), r.r(4)), (5, (-3i32) as u32));
    // Z: operands differ; N: -3-5 < 0; C: src2 selected; V: no overflow.
    assert_eq!(r.znvc(), (false, true, true, false));
}

// ---- multiply --------------------------------------------------------------

#[test]
fn mul64_results_leapster_mmid() {
    // 0x12345678 * 0x9abcdef0, signed and unsigned (values from exact
    // integer arithmetic). MMID = low word under MmidMode::LowLeapster, and
    // r57 (the dual MAC's accumulator there, see exec.rs MUL64) is unchanged.
    let r = run("t_mul64");
    assert_eq!((r.r(10), r.r(11), r.r(12)), (0, 0x242D_2080, 0xF8CC_93D6));
    assert_eq!((r.r(13), r.r(14), r.r(15)), (0, 0x242D_2080, 0x0B00_EA4E));
}

#[test]
fn mul64_results_prm_mmid() {
    let mut cfg = CpuConfig::leapster();
    cfg.mmid = MmidMode::Middle;
    let mut r = Rig::new(cfg);
    r.cpu.pc = sym("t_mul64");
    r.run();
    assert_eq!(r.r(11), 0x93D6_242D);
    assert_eq!(r.r(14), 0xEA4E_242D);
}

#[test]
fn mulhi_restores_mhi() {
    assert_eq!(run("t_mulhi").r(1), 0xDEAD_BEEF);
}

// ---- memory (PRM: LD, ST, write-back modes, PUSH_S/POP_S, GP/PCL) -----------

#[test]
fn loads_stores_extension_and_writeback() {
    let r = run("t_loadstore");
    assert_eq!((r.r(3), r.r(4), r.r(5)), (0x44, 0x11, 0x1122), "little-endian sub-word loads");
    assert_eq!((r.r(7), r.r(8)), (0xFFFF_8080, 0xFFFF_FF80), "sign extension");
    assert_eq!(r.r(9), 0x1122_3344, ".as scales the offset by the access size");
    assert_eq!(r.r(10), 0x3C00_010C, ".ab then .a write-back");
}

#[test]
fn push_pop_round_trip() {
    let r = run("t_pushpop");
    assert_eq!((r.r(1), r.r(reg::BLINK), r.r(3)), (7, 0x1234, SP));
}

#[test]
fn gp_relative_load() {
    assert_eq!(run("t_gp").r(0), 0x5555_AAAA);
}

#[test]
fn pcl_relative_load_uses_aligned_pc() {
    assert_eq!(run("t_pcl").r(1), 0xCAFE_F00D);
}

// ---- control flow -----------------------------------------------------------

#[test]
fn delay_slots() {
    // b.d: slot runs; b: following instruction killed; beq.d not taken: slot
    // and fall-through both run (PRM Table 51).
    let r = run("t_bdelay");
    assert_eq!((r.r(1), r.r(2)), (1101, 0));
}

#[test]
fn branch_and_link_return_addresses() {
    let r = run("t_bl");
    assert_eq!(r.r(5), sym("ret1"), "BL: BLINK = next instruction");
    assert_eq!(r.r(7), sym("ret2"), "BL.D: BLINK = instruction after the delay slot");
    assert_eq!(r.r(6), 1);
}

#[test]
fn compare_and_branch_variants() {
    assert_eq!(run("t_brcc").r(2), 100_110);
}

#[test]
fn jumps_and_jump_and_link() {
    let r = run("t_jumps");
    assert_eq!(r.r(2), 1);
    assert_eq!(r.r(4), sym("jl_ret"));
}

#[test]
fn zero_overhead_loop_runs_lp_count_times() {
    let r = run("t_loop");
    assert_eq!(r.r(1), 55);
    assert_eq!(r.r(reg::LP_COUNT), 0);
}

#[test]
fn false_lpcc_jumps_to_loop_end() {
    assert_eq!(run("t_loop_skipped").r(1), 0);
}

// ---- auxiliary registers, FLAG, interrupts ---------------------------------

#[test]
fn auxiliary_registers() {
    let r = run("t_aux");
    let id = CpuConfig::leapster().identity;
    assert_eq!(r.r(1), id);
    assert_eq!(r.r(3), 0x0001_2000, "INT_VECTOR_BASE ignores the low 10 bits");
    assert_eq!(r.r(4), status32::Z);
    assert_eq!(r.r(6), 0x4000, "LP_START bit 0 reads as 0");
    assert_eq!(r.r(7), id, "unknown aux reads return IDENTITY on the A5");
    assert_eq!(r.cpu.diag.aux_unmodelled_total, 1);
}

#[test]
fn flag_sets_status_and_halts() {
    assert_eq!(run("t_flag").r(1), 0x806);
    let mut r = Rig::at("t_flag_halt");
    assert_eq!(r.run(), HaltReason::Flag);
    assert!(r.cpu.halted());
    assert!(matches!(r.cpu.step(&mut r.bus), Err(CpuError::Halted { .. })));
}

#[test]
fn level1_interrupt_entry_wakes_sleep_and_returns() {
    let mut r = Rig::at("t_irq");
    r.run_until_sleep();
    assert!(r.cpu.sleeping);
    r.cpu.set_irq_line(16, true);
    assert_eq!(r.cpu.step(&mut r.bus).unwrap(), Step::Interrupt { vector: 16 });
    assert!(!r.cpu.sleeping);
    assert_eq!(r.cpu.pc, 0x4000_0000 + 16 * 8);
    assert_eq!(r.r(reg::ILINK1), sym("irq_resume"), "ILINK1 = instruction after SLEEP");
    assert_eq!(r.cpu.status32 & status32::E1, 0);
    assert_eq!(r.cpu.status32_l1 & status32::E1, status32::E1);
    assert_eq!(r.cpu.aux_irq_lv12, 1);
    assert_eq!(r.run(), HaltReason::Brk); // inside the handler
    assert_eq!(r.r(20), sym("irq_resume"));
    r.cpu.set_irq_line(16, false);
    r.resume_after_brk();
    assert_eq!(r.run(), HaltReason::Brk);
    assert_eq!(r.r(9), 1);
    assert_eq!(r.cpu.status32 & status32::E1, status32::E1, "J.F [ILINK1] restored STATUS32");
}

#[test]
fn masked_interrupt_does_not_wake_or_enter() {
    let mut r = Rig::at("t_irq");
    r.run_until_sleep();
    r.cpu.status32 &= !status32::E1;
    r.cpu.set_irq_line(16, true);
    assert_eq!(r.cpu.step(&mut r.bus).unwrap(), Step::Sleeping);
}

#[test]
fn swi_enters_instruction_error_vector() {
    let r = run("t_swi");
    assert_eq!(r.r(21), sym("swi_ret"), "ILINK2 = instruction after SWI");
    assert_eq!(r.cpu.pc, sym("swi_ret"));
}

#[test]
fn timer0_interrupt_wakes_sleep() {
    let r = run("t_timer");
    assert_eq!(r.r(23), sym("timer_resume"));
    assert!(!r.cpu.timers[0].pending, "writing CONTROL0 clears the interrupt");
}

#[test]
fn level2_preempts_and_priority_order() {
    let mut cpu = Cpu::new(CpuConfig::leapster());
    cpu.status32 |= status32::E1 | status32::E2;
    cpu.set_irq_line(5, true);
    cpu.set_irq_line(20, true);
    assert_eq!(cpu.pending_interrupt(), Some(20), "higher IRQ number wins within level 1");
    cpu.set_irq_line(6, true);
    assert_eq!(cpu.pending_interrupt(), Some(6), "level 2 (IRQ6 by default) first");
    cpu.set_irq_line(7, true);
    assert_eq!(cpu.pending_interrupt(), Some(7), "IRQ7 highest in level 2");
}

// ---- optional extensions -----------------------------------------------------

#[test]
fn norm_swap_and_saturating_arithmetic() {
    let r = run("t_options");
    assert_eq!((r.r(2), r.r(3), r.r(4)), (2, 0x1F, 0x5678_1234));
    assert_eq!(r.r(5), 0x7FFF_FFFF);
    assert_eq!(r.r(6), 0x8000_0000);
    assert_eq!(r.r(7), 0x7FFF_0002);
    assert_eq!(r.r(9), 0x7FFF);
    assert_eq!((r.r(10), r.r(11)), (0x7FFF_FFFF, 0x7FFF_FFFF));
    assert_eq!(r.r(12), 0x7FFF_FFFF);
    assert_eq!((r.r(13), r.r(14)), (0xFFFF_FFFC, 0xFFFF_FFFC));
}

#[test]
fn dual_16x16_multiply_accumulate() {
    // SOURCED (MAME `dsp_multiply`), UNVERIFIED: no specification of the dual
    // 16x16 MAC option is available (see exec.rs). AUX_MACMODE = 8: Q15
    // results (shift 15). Each lane: 0x4000 * 0x2000 = 2^27.
    let r = run("t_dsp");
    assert_eq!(r.r(3), 0x1000_1000, "MULDW: 2^27 >> 15 per lane");
    assert_eq!(r.r(4), 0x2000_2000, "MACDW accumulates");
    assert_eq!(r.r(5), 0x1000_1000, "MSUBDW subtracts");
    assert_eq!(r.r(7), 0x7FFE_7FFE, "0x7FFF^2 >> 15");
    assert_eq!(r.r(8), 0x7FFF_7FFF, "twice that saturates in both lanes");
    assert_eq!(r.r(9) & 0x120, 0x120, "sticky S1 (high) and S2 (low) set");
    // AUX_MACMODE = 0: shift 16. High 0x7FFF^2 >> 16 = 0x3FFF; low
    // -0x8000 * 0x7FFF = -0x3FFF_8000, >> 16 = -0x4000 (floor).
    assert_eq!(r.r(10), 0x3FFF_C000);
    // RND16 (PRM instruction page): 0x1234_8000 rounds up to 0x1235; near the
    // top it saturates first (0x7FFF_FFFF) and gives 0x7FFF.
    assert_eq!((r.r(11), r.r(12)), (0x1235, 0x7FFF));
    // The accumulators read as r56 (high lane) / r57 (low lane), VERIFIED-FW
    // use (exec.rs, MULDW). After the last MULDW (AUX_MACMODE 0, integer):
    // 0x7FFF^2 and -0x8000 * 0x7FFF.
    assert_eq!((r.r(56), r.r(57)), (0x3FFF_0001, 0xC000_8000));
}

#[test]
fn dual_mac_accumulators_read_back_in_fractional_mode() {
    // AUX_MACMODE = 8 (fractional): products are doubled (Q31). Stop right
    // after `muldw r3,r1,r2` (0x4000 * 0x2000 per lane).
    let mut r = Rig::at("t_dsp");
    while r.r(3) != 0x1000_1000 {
        r.cpu.step(&mut r.bus).unwrap();
    }
    assert_eq!((r.r(56), r.r(57)), (0x1000_0000, 0x1000_0000), "2 * 2^27");
    // A write to r57 sets the low lane's accumulator: MACDW adds to it.
    r.cpu.regs[57] = 0;
    r.cpu.mac_acc[0] = 0;
    r.cpu.step(&mut r.bus).unwrap(); // macdw r4,r1,r2
    assert_eq!((r.r(4), r.r(57)), (0x2000_1000, 0x1000_0000), "low lane restarted from 0");
}

// ---- diagnostics: nothing silently becomes a no-op ---------------------------

fn halves(words: &[u16]) -> Vec<u8> {
    words.iter().flat_map(|h| h.to_le_bytes()).collect()
}

#[test]
fn illegal_instruction_stops_without_side_effects() {
    let w: u32 = (0x04 << 27) | (0x1E << 16); // major 0x04, reserved sub-opcode
    let mut bus = image_bus(&halves(&[(w >> 16) as u16, w as u16, 0, 0]));
    let mut cpu = Cpu::new(CpuConfig::leapster());
    let before = cpu.clone();
    let err = cpu.step(&mut bus).unwrap_err();
    assert!(matches!(err, CpuError::Illegal { pc: 0x4000_0000, .. }), "{err}");
    assert_eq!(cpu, before);
}

#[test]
fn illegal_instruction_can_raise_instruction_error() {
    let w: u32 = (0x04 << 27) | (0x1E << 16);
    let mut bus = image_bus(&halves(&[(w >> 16) as u16, w as u16, 0, 0]));
    let mut cfg = CpuConfig::leapster();
    cfg.illegal = IllegalPolicy::Exception;
    let mut cpu = Cpu::new(cfg);
    assert!(matches!(cpu.step(&mut bus).unwrap(), Step::Executed));
    assert!(matches!(cpu.last_instr.unwrap().op, Op::Illegal(_)));
    assert_eq!(cpu.pc, 0x4000_0010);
    assert_eq!(cpu.regs[reg::ILINK2], 0x4000_0004);
}

#[test]
fn extension_register_is_reported_when_xy_extension_is_absent() {
    // mov r1,r40 (r40 = y0_u0 in binutils' naming).
    let w: u32 = (0x04 << 27) | (1 << 24) | (0x0A << 16) | (40 << 6);
    let mut bus = image_bus(&halves(&[(w >> 16) as u16, w as u16]));
    let mut cfg = CpuConfig::leapster();
    cfg.xy_extension = false;
    let mut cpu = Cpu::new(cfg);
    let before = cpu.clone();
    let err = cpu.step(&mut bus).unwrap_err();
    assert!(matches!(&err, CpuError::Unsupported { info, .. } if info.reason.contains("r40")), "{err}");
    assert_eq!(cpu, before);
}

#[test]
fn xy_port_reads_y_memory_through_its_address_generator() {
    // mov r1,r40 with AY0 = 5 and MY00 = +1: reads Y word 5, AY0 becomes 6.
    let w: u32 = (0x04 << 27) | (1 << 24) | (0x0A << 16) | (40 << 6);
    let mut bus = image_bus(&halves(&[(w >> 16) as u16, w as u16]));
    let mut cpu = Cpu::new(CpuConfig::leapster());
    let xy = cpu.xy.as_mut().unwrap();
    xy.aux[4] = 5; // AY0
    xy.aux[8 + 4 * 2] = 1; // MY00
    xy.mem[0][1][5] = 0xFEED_F00D;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[1], 0xFEED_F00D);
    assert_eq!(cpu.xy.as_ref().unwrap().aux[4], 6);
}

#[test]
fn branch_in_delay_slot_is_unpredictable() {
    // b.d +8 (B far, N=1) followed by b_s 0.
    let mut bus = image_bus(&halves(&[0x0009, 0x0020, 0xF000, 0x78E0]));
    let mut cpu = Cpu::new(CpuConfig::leapster());
    assert!(matches!(cpu.step(&mut bus).unwrap(), Step::Executed));
    assert!(matches!(cpu.step(&mut bus), Err(CpuError::Unpredictable { .. })));
}

#[test]
fn execution_is_deterministic() {
    let a = run("t_loop");
    let b = run("t_loop");
    assert_eq!(a.cpu, b.cpu);
    assert_eq!(a.cpu.cycles, a.cpu.instructions_retired, "placeholder timing: 1 cycle per instruction");
}

/// Pipeline timing (`CpuConfig::pipeline_timing`, ARCompact PRM costs for the
/// ARCtangent-A5), on hand-encoded 16-bit instructions (checked against the
/// decoder here and against Ribbit's disassembler).
#[test]
fn pipeline_costs_load_use_and_taken_compare_branch() {
    let prog: [u16; 5] = [
        0x8100, // ld_s r0,[r1,0]
        0x7058, // mov_s r2,r0: reads the register just loaded
        0xE802, // breq_s r0,0,+4: taken, no delay slot, compare-and-branch
        0x78E0, // nop_s (skipped)
        0x78E0, // nop_s
    ];
    let bytes: Vec<u8> = prog.iter().flat_map(|h| h.to_le_bytes()).collect();
    let costs = |timing: bool, latency: u64| {
        let mut config = CpuConfig::leapster();
        config.pipeline_timing = timing;
        config.load_latency = latency;
        let mut cpu = Cpu::new(config);
        let mut bus = image_bus(&bytes);
        cpu.pc = 0x4000_0000;
        cpu.regs[1] = SP; // RAM, reads as 0
        let mut out = vec![];
        for _ in 0..4 {
            let (before, pc) = (cpu.cycles, cpu.pc);
            assert!(matches!(cpu.step(&mut bus), Ok(Step::Executed)), "{:?}", cpu);
            out.push((pc, cpu.last_instr.unwrap().op, cpu.cycles - before));
        }
        out
    };
    // A result arriving `latency` cycles after its load issues: the next
    // instruction (one cycle later) waits latency - 1.
    for latency in [2, 3] {
        let on = costs(true, latency);
        assert_eq!(on.iter().map(|c| (c.0, c.2)).collect::<Vec<_>>(), [
            (0x4000_0000, 1),
            (0x4000_0002, latency), // load-use stall
            (0x4000_0004, 3), // killed delay slot + the A5's second slot
            (0x4000_0008, 1),
        ]);
        assert!(matches!(on[0].1, Op::Ld) && matches!(on[1].1, Op::Mov) && matches!(on[2].1, Op::Br(_)), "{on:?}");
    }
    assert!(costs(false, 3).iter().all(|c| c.2 == 1), "one cycle each without pipeline timing");
}
