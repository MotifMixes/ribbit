//! An MCS-51 disassembler (Intel mnemonics), for traces and research tools.

/// Names of the standard 8052 SFRs; other addresses print as hex.
pub fn sfr_name(addr: u8) -> Option<&'static str> {
    Some(match addr {
        0x80 => "P0",
        0x81 => "SP",
        0x82 => "DPL",
        0x83 => "DPH",
        0x87 => "PCON",
        0x88 => "TCON",
        0x89 => "TMOD",
        0x8A => "TL0",
        0x8B => "TL1",
        0x8C => "TH0",
        0x8D => "TH1",
        0x90 => "P1",
        0x98 => "SCON",
        0x99 => "SBUF",
        0xA0 => "P2",
        0xA8 => "IE",
        0xB0 => "P3",
        0xB8 => "IP",
        0xC8 => "T2CON",
        0xCA => "RCAP2L",
        0xCB => "RCAP2H",
        0xCC => "TL2",
        0xCD => "TH2",
        0xD0 => "PSW",
        0xE0 => "ACC",
        0xF0 => "B",
        _ => return None,
    })
}

fn direct(a: u8) -> String {
    match sfr_name(a) {
        Some(n) => n.to_string(),
        None => format!("{a:02X}h"),
    }
}

fn bit(b: u8) -> String {
    if b < 0x80 {
        format!("{:02X}h.{}", 0x20 + (b >> 3), b & 7)
    } else {
        format!("{}.{}", direct(b & 0xF8), b & 7)
    }
}

/// The length in bytes of the instruction starting with `op`.
pub fn length(op: u8) -> u16 {
    match op {
        o if o & 0x1F == 0x01 || o & 0x1F == 0x11 => 2,
        0x02 | 0x12 | 0x10 | 0x20 | 0x30 | 0x43 | 0x53 | 0x63 | 0x75 | 0x85 | 0x90 | 0xB4..=0xBF | 0xD5 => 3,
        0x05 | 0x15 | 0x24 | 0x25 | 0x34 | 0x35 | 0x40 | 0x42 | 0x44 | 0x45 | 0x50 | 0x52 | 0x54 | 0x55 | 0x60
        | 0x62 | 0x64 | 0x65 | 0x70 | 0x72 | 0x74 | 0x76 | 0x77 | 0x78..=0x7F | 0x80 | 0x82 | 0x86..=0x8F | 0x92
        | 0x94 | 0x95 | 0xA0 | 0xA2 | 0xA6..=0xAF | 0xB0 | 0xB2 | 0xC0 | 0xC2 | 0xC5 | 0xD0 | 0xD2 | 0xD8..=0xDF
        | 0xE5 | 0xF5 => 2,
        _ => 1,
    }
}

/// Disassembles the instruction at `pc` (bytes from `read`), returning its
/// text and length. Jump targets are absolute.
pub fn disasm(pc: u16, mut read: impl FnMut(u16) -> u8) -> (String, u16) {
    let op = read(pc);
    let len = length(op);
    let b1 = read(pc.wrapping_add(1));
    let b2 = read(pc.wrapping_add(2));
    let next = pc.wrapping_add(len);
    let rel = |r: u8| next.wrapping_add(r as i8 as i16 as u16);
    let src = |op: u8| -> String {
        match op & 0xF {
            4 => format!("#{b1:02X}h"),
            5 => direct(b1),
            6 | 7 => format!("@R{}", op & 1),
            n => format!("R{}", n - 8),
        }
    };
    let text = match op {
        0x00 => "NOP".into(),
        o if o & 0x1F == 0x01 || o & 0x1F == 0x11 => {
            let t = (next & 0xF800) | (((o as u16) >> 5) << 8) | b1 as u16;
            format!("{} {t:04X}", if o & 0x10 != 0 { "ACALL" } else { "AJMP" })
        }
        0x02 => format!("LJMP {:04X}", u16::from_be_bytes([b1, b2])),
        0x12 => format!("LCALL {:04X}", u16::from_be_bytes([b1, b2])),
        0x03 => "RR A".into(),
        0x13 => "RRC A".into(),
        0x23 => "RL A".into(),
        0x33 => "RLC A".into(),
        0x04 => "INC A".into(),
        0x14 => "DEC A".into(),
        0x05 => format!("INC {}", direct(b1)),
        0x15 => format!("DEC {}", direct(b1)),
        0x06 | 0x07 => format!("INC @R{}", op & 1),
        0x16 | 0x17 => format!("DEC @R{}", op & 1),
        0x08..=0x0F => format!("INC R{}", op - 8),
        0x18..=0x1F => format!("DEC R{}", op - 0x18),
        0x10 => format!("JBC {},{:04X}", bit(b1), rel(b2)),
        0x20 => format!("JB {},{:04X}", bit(b1), rel(b2)),
        0x30 => format!("JNB {},{:04X}", bit(b1), rel(b2)),
        0x22 => "RET".into(),
        0x32 => "RETI".into(),
        0x24..=0x2F => format!("ADD A,{}", src(op)),
        0x34..=0x3F => format!("ADDC A,{}", src(op)),
        0x40 => format!("JC {:04X}", rel(b1)),
        0x50 => format!("JNC {:04X}", rel(b1)),
        0x60 => format!("JZ {:04X}", rel(b1)),
        0x70 => format!("JNZ {:04X}", rel(b1)),
        0x80 => format!("SJMP {:04X}", rel(b1)),
        0x42 | 0x52 | 0x62 => format!("{} {},A", ["ORL", "ANL", "XRL"][(op as usize >> 4) - 4], direct(b1)),
        0x43 | 0x53 | 0x63 => {
            format!("{} {},#{b2:02X}h", ["ORL", "ANL", "XRL"][(op as usize >> 4) - 4], direct(b1))
        }
        0x44..=0x4F | 0x54..=0x5F | 0x64..=0x6F => {
            format!("{} A,{}", ["ORL", "ANL", "XRL"][(op as usize >> 4) - 4], src(op))
        }
        0x72 => format!("ORL C,{}", bit(b1)),
        0x82 => format!("ANL C,{}", bit(b1)),
        0xA0 => format!("ORL C,/{}", bit(b1)),
        0xB0 => format!("ANL C,/{}", bit(b1)),
        0x73 => "JMP @A+DPTR".into(),
        0x74 => format!("MOV A,#{b1:02X}h"),
        0x75 => format!("MOV {},#{b2:02X}h", direct(b1)),
        0x76 | 0x77 => format!("MOV @R{},#{b1:02X}h", op & 1),
        0x78..=0x7F => format!("MOV R{},#{b1:02X}h", op - 0x78),
        0x83 => "MOVC A,@A+PC".into(),
        0x84 => "DIV AB".into(),
        0x85 => format!("MOV {},{}", direct(b2), direct(b1)),
        0x86 | 0x87 => format!("MOV {},@R{}", direct(b1), op & 1),
        0x88..=0x8F => format!("MOV {},R{}", direct(b1), op - 0x88),
        0x90 => format!("MOV DPTR,#{:04X}h", u16::from_be_bytes([b1, b2])),
        0x92 => format!("MOV {},C", bit(b1)),
        0x93 => "MOVC A,@A+DPTR".into(),
        0x94..=0x9F => format!("SUBB A,{}", src(op)),
        0xA2 => format!("MOV C,{}", bit(b1)),
        0xA3 => "INC DPTR".into(),
        0xA4 => "MUL AB".into(),
        0xA5 => "DB A5h".into(),
        0xA6 | 0xA7 => format!("MOV @R{},{}", op & 1, direct(b1)),
        0xA8..=0xAF => format!("MOV R{},{}", op - 0xA8, direct(b1)),
        0xB2 => format!("CPL {}", bit(b1)),
        0xB3 => "CPL C".into(),
        0xB4 => format!("CJNE A,#{b1:02X}h,{:04X}", rel(b2)),
        0xB5 => format!("CJNE A,{},{:04X}", direct(b1), rel(b2)),
        0xB6 | 0xB7 => format!("CJNE @R{},#{b1:02X}h,{:04X}", op & 1, rel(b2)),
        0xB8..=0xBF => format!("CJNE R{},#{b1:02X}h,{:04X}", op - 0xB8, rel(b2)),
        0xC0 => format!("PUSH {}", direct(b1)),
        0xD0 => format!("POP {}", direct(b1)),
        0xC2 => format!("CLR {}", bit(b1)),
        0xD2 => format!("SETB {}", bit(b1)),
        0xC3 => "CLR C".into(),
        0xD3 => "SETB C".into(),
        0xC4 => "SWAP A".into(),
        0xC5 => format!("XCH A,{}", direct(b1)),
        0xC6 | 0xC7 => format!("XCH A,@R{}", op & 1),
        0xC8..=0xCF => format!("XCH A,R{}", op - 0xC8),
        0xD4 => "DA A".into(),
        0xD5 => format!("DJNZ {},{:04X}", direct(b1), rel(b2)),
        0xD6 | 0xD7 => format!("XCHD A,@R{}", op & 1),
        0xD8..=0xDF => format!("DJNZ R{},{:04X}", op - 0xD8, rel(b1)),
        0xE0 => "MOVX A,@DPTR".into(),
        0xE2 | 0xE3 => format!("MOVX A,@R{}", op & 1),
        0xE4 => "CLR A".into(),
        0xE5 => format!("MOV A,{}", direct(b1)),
        0xE6 | 0xE7 => format!("MOV A,@R{}", op & 1),
        0xE8..=0xEF => format!("MOV A,R{}", op - 0xE8),
        0xF0 => "MOVX @DPTR,A".into(),
        0xF2 | 0xF3 => format!("MOVX @R{},A", op & 1),
        0xF4 => "CPL A".into(),
        0xF5 => format!("MOV {},A", direct(b1)),
        0xF6 | 0xF7 => format!("MOV @R{},A", op & 1),
        0xF8..=0xFF => format!("MOV R{},A", op - 0xF8),
        _ => format!("DB {op:02X}h"),
    };
    (text, len)
}
