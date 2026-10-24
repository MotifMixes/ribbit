//! MCS-51 instruction checks against the flag and result rules of Intel's
//! MCS-51 user's manual, on a flat 64 KB code + 64 KB XDATA test bus.

use ribbit_leappad::mcs51::{self, Bus, Mcs51};

struct Flat {
    code: Vec<u8>,
    x: Vec<u8>,
    sent: Vec<u8>,
}

impl Bus for Flat {
    fn code(&mut self, a: u16) -> u8 {
        self.code[a as usize]
    }
    fn xread(&mut self, a: u16) -> u8 {
        self.x[a as usize]
    }
    fn xwrite(&mut self, a: u16, v: u8) {
        self.x[a as usize] = v;
    }
    fn sfr_read(&mut self, _: u8) -> u8 {
        0
    }
    fn sfr_write(&mut self, _: u8, _: u8) {}
    fn serial_tx(&mut self, b: u8) {
        self.sent.push(b);
    }
}

/// Runs `prog` from address 0 for `steps` instructions.
fn run(prog: &[u8], steps: usize) -> (Mcs51, Flat) {
    let mut code = vec![0u8; 0x10000];
    code[..prog.len()].copy_from_slice(prog);
    let mut bus = Flat { code, x: vec![0; 0x10000], sent: Vec::new() };
    let mut cpu = Mcs51::new();
    for _ in 0..steps {
        cpu.step(&mut bus);
    }
    (cpu, bus)
}

const CY: u8 = 0x80;
const AC: u8 = 0x40;
const OV: u8 = 0x04;

#[test]
fn add_sets_carry_aux_and_overflow() {
    // MOV A,#7Fh; ADD A,#01h -> 80h, OV and AC set, CY clear.
    let (c, _) = run(&[0x74, 0x7F, 0x24, 0x01], 2);
    assert_eq!(c.acc(), 0x80);
    assert_eq!(c.psw() & (CY | AC | OV), AC | OV);
    // MOV A,#FFh; ADD A,#01h -> 00h, CY and AC, no OV; parity of 0 is even.
    let (c, _) = run(&[0x74, 0xFF, 0x24, 0x01], 2);
    assert_eq!(c.acc(), 0x00);
    assert_eq!(c.psw() & (CY | AC | OV | 1), CY | AC);
}

#[test]
fn addc_and_subb_use_the_carry() {
    // SETB C; MOV A,#10h; ADDC A,#05h -> 16h.
    let (c, _) = run(&[0xD3, 0x74, 0x10, 0x34, 0x05], 3);
    assert_eq!(c.acc(), 0x16);
    // SETB C; MOV A,#10h; SUBB A,#05h -> 0Ah, AC (borrow from bit 4), no CY.
    let (c, _) = run(&[0xD3, 0x74, 0x10, 0x94, 0x05], 3);
    assert_eq!(c.acc(), 0x0A);
    assert_eq!(c.psw() & (CY | AC | OV), AC);
    // CLR C; MOV A,#80h; SUBB A,#01h -> 7Fh with OV (negative minus positive).
    let (c, _) = run(&[0xC3, 0x74, 0x80, 0x94, 0x01], 3);
    assert_eq!(c.acc(), 0x7F);
    assert_eq!(c.psw() & (CY | OV), OV);
    // CLR C; MOV A,#00h; SUBB A,#01h -> FFh with CY.
    let (c, _) = run(&[0xC3, 0x74, 0x00, 0x94, 0x01], 3);
    assert_eq!((c.acc(), c.psw() & CY), (0xFF, CY));
}

#[test]
fn decimal_adjust() {
    // MOV A,#56h; ADD A,#67h; DA A -> 23h with CY (56 + 67 = 123 BCD).
    let (c, _) = run(&[0x74, 0x56, 0x24, 0x67, 0xD4], 3);
    assert_eq!(c.acc(), 0x23);
    assert_eq!(c.psw() & CY, CY);
    // MOV A,#09h; ADD A,#01h; DA A -> 10h.
    let (c, _) = run(&[0x74, 0x09, 0x24, 0x01, 0xD4], 3);
    assert_eq!(c.acc(), 0x10);
}

#[test]
fn mul_and_div() {
    // MOV A,#50h; MOV B,#A0h; MUL AB -> 3200h: A=00, B=32, OV.
    let (c, _) = run(&[0x74, 0x50, 0x75, 0xF0, 0xA0, 0xA4], 3);
    assert_eq!((c.acc(), c.peek_sfr(mcs51::B)), (0x00, 0x32));
    assert_eq!(c.psw() & (CY | OV), OV);
    // MOV A,#FBh; MOV B,#12h; DIV AB -> A=0Dh, B=11h.
    let (c, _) = run(&[0x74, 0xFB, 0x75, 0xF0, 0x12, 0x84], 3);
    assert_eq!((c.acc(), c.peek_sfr(mcs51::B)), (0x0D, 0x11));
    // Division by zero sets OV.
    let (c, _) = run(&[0x74, 0x05, 0x75, 0xF0, 0x00, 0x84], 3);
    assert_eq!(c.psw() & OV, OV);
}

#[test]
fn register_banks_and_indirect() {
    // MOV PSW,#08h (bank 1); MOV R0,#40h; MOV @R0,#5Ah; MOV A,40h.
    let (c, _) = run(&[0x75, 0xD0, 0x08, 0x78, 0x40, 0x76, 0x5A, 0xE5, 0x40], 4);
    assert_eq!(c.iram[0x08], 0x40, "R0 of bank 1 is at 08h");
    assert_eq!(c.acc(), 0x5A);
    // Indirect access reaches 80h-FFh as RAM, not SFRs (8052).
    let (c, _) = run(&[0x78, 0x90, 0x76, 0x33, 0xE6], 3);
    assert_eq!((c.iram[0x90], c.acc()), (0x33, 0x33));
}

#[test]
fn calls_returns_and_jumps() {
    // 0000: LCALL 0010; 0003: SJMP $ ; 0010: MOV A,#77h; RET
    let mut p = vec![0x12, 0x00, 0x10, 0x80, 0xFE];
    p.resize(0x10, 0);
    p.extend_from_slice(&[0x74, 0x77, 0x22]);
    let (c, _) = run(&p, 4);
    assert_eq!((c.acc(), c.pc, c.sp()), (0x77, 0x0003, 0x07));
    // AJMP within the 2 KB page: 0000: AJMP 0123 (21 23)
    let (c, _) = run(&[0x21, 0x23], 1);
    assert_eq!(c.pc, 0x0123);
    // CJNE A,#05h sets CY when A < 05h, and jumps when not equal.
    let (c, _) = run(&[0x74, 0x03, 0xB4, 0x05, 0x10], 2);
    assert_eq!((c.pc, c.psw() & CY), (0x0005 + 0x10, CY));
    // DJNZ R2 counts down to zero: MOV R2,#03h; loop: DJNZ R2,loop
    let (c, _) = run(&[0x7A, 0x03, 0xDA, 0xFE], 4);
    assert_eq!((c.r(2), c.pc), (0, 4));
}

#[test]
fn bit_operations() {
    // SETB 20h.3 (bit 03h); MOV C,03h; CPL C; JBC 03h,+2
    let (c, _) = run(&[0xD2, 0x03, 0xA2, 0x03, 0xB3, 0x10, 0x03, 0x02], 4);
    assert_eq!(c.iram[0x20] & 0x08, 0, "JBC cleared the bit");
    assert_eq!(c.pc, 0x0008 + 2, "and jumped");
    assert_eq!(c.psw() & CY, 0);
}

#[test]
fn movc_and_movx() {
    // MOV DPTR,#0100h; MOV A,#02h; MOVC A,@A+DPTR; MOVX @DPTR,A
    let mut p = vec![0x90, 0x01, 0x00, 0x74, 0x02, 0x93, 0xF0];
    p.resize(0x100, 0);
    p.extend_from_slice(&[0x11, 0x22, 0x33]);
    let (c, b) = run(&p, 4);
    assert_eq!(c.acc(), 0x33);
    assert_eq!(b.x[0x100], 0x33);
}

#[test]
fn timer0_overflow_interrupt() {
    // TMOD=02 (timer 0 mode 2), TH0=TL0=FEh, IE=82h, TR0=1, then spin.
    // Vector 000Bh: MOV A,#99h; SJMP $
    let mut p = vec![0x75, 0x89, 0x02, 0x75, 0x8C, 0xFE, 0x75, 0x8A, 0xFE, 0x75, 0xA8, 0x82, 0xD2, 0x8C, 0x80, 0xFE];
    p.resize(0x0B, 0);
    p[0..3].copy_from_slice(&[0x02, 0x00, 0x20]); // LJMP 0020
    p.resize(0x20, 0);
    let main = [0x75, 0x89, 0x02, 0x75, 0x8C, 0xFE, 0x75, 0x8A, 0xFE, 0x75, 0xA8, 0x82, 0xD2, 0x8C, 0x80, 0xFE];
    p.extend_from_slice(&main);
    p[0x0B] = 0x74;
    p[0x0C] = 0x99;
    p[0x0D] = 0x80;
    p[0x0E] = 0xFE;
    let (c, _) = run(&p, 20);
    assert_eq!(c.acc(), 0x99, "the timer 0 interrupt was taken");
    assert_eq!(c.in_service, 1);
}

#[test]
fn serial_transmit_sets_ti() {
    // MOV SBUF,#41h; loop: JNB TI,loop
    let (c, b) = run(&[0x75, 0x99, 0x41, 0x30, 0x99, 0xFD], 200);
    assert_eq!(b.sent, vec![0x41]);
    assert_eq!(c.peek_sfr(mcs51::SCON) & 0x02, 0x02);
    assert!(c.pc > 5, "left the wait loop");
}

#[test]
fn disassembler_lengths_cover_every_opcode() {
    use ribbit_leappad::disasm::{disasm, length};
    for op in 0..=255u8 {
        let (_, len) = disasm(0, |a| if a == 0 { op } else { 0 });
        assert_eq!(len, length(op), "opcode {op:02X}");
        assert!((1..=3).contains(&len));
    }
}
