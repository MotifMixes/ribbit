//! Leapster 2: the SD card slot (pin block 0x0180_00E0-00F8), its host
//! controller (0x0180_E400-E408) and the card.
//!
//! VERIFIED-FW (BIOS 0.6 `152-12206`; the SD driver names itself in its
//! strings: "gSDDMAContextPtr", "ResetAndGetToStandbyState", "SectorRead1",
//! "cmd17 (Read) Failed to run", "!! Card Not Inserted !!").
//!
//! Pin block:
//!
//! | Register | Use by the BIOS | Meaning |
//! |---|---|---|
//! | 0x0180_00EC | read only. Card detect 0x400C_ED54: `(EC >> 6 & 1) ^ 1`; write-busy poll 0x400C_F766 loops while bit 1 is 0 and stops with "Card Ejected during write poll" if bit 6 is set | pin inputs: bit 6 = card absent, bit 1 = DAT0 level |
//! | 0x0180_00F0 | 0x7F normally; 0x7D (bit 1 clear) around the write-busy poll (0x400C_E824 / 0x400C_E83C) | which SD lines the controller drives (HYPOTHESIS: bit 1 = DAT0, released so its level can be read) |
//! | 0x0180_00E0, 00E4, 00E8, 00F4, 00F8 | written at init (0x400C_E7C0) and power-down (0x400C_E854) | UNKNOWN |
//!
//! Host controller: 0x0180_E404 = address of a command descriptor in RAM,
//! 0x0180_E400 = write 1 to run it, then polled until it reads 0 (at most
//! 100,000 reads, 0x400C_E884), 0x0180_E408 = clock and bus mode
//! (0x7700_0001 during identification, 1 or 0x0100_0001 after; HYPOTHESIS:
//! slow clock, fast clock, 4-bit bus). Descriptor (built at 0x400C_E970 and,
//! with data, 0x400C_EA84):
//!
//! | Offset | Contents |
//! |---|---|
//! | +0x00 u16 | status, 0xFFFF before. After: bit 0 = an error (not a missing response, see `run`); bits 4-7 = data lines whose read CRC was good (0xF0 on a 4-bit bus, 0x10 on 1 bit, checked by SectorRead1 0x400C_F5E0); bits 12-15 = the write CRC token (SectorWrite1 0x400C_F79C wants `& 0xB000 == 0x2000`) |
//! | +0x02 u8 | type: bit 3 = 48-bit response, bit 2 = 136-bit response, bit 0 = data from the card, bit 1 = data to the card, bit 6 = 4-bit bus (CMD17 0x09, CMD24 0x0A) |
//! | +0x03 u8 | 0x80 = active; the controller clears it ("DMA Active bit still set" otherwise) |
//! | +0x04 u16 | `(0x40 \| cmd) << 8 \| arg[31:24]`; +0x08 u32 `arg[23:0] << 8` (the command token) |
//! | +0x0C | data form: u16 byte count (0x200), +0x10 the buffer; then a zero-length terminating segment and a self pointer (no-data form: the terminator at +0x0C) |
//! | +0x18 (+0x20 with data) | 16 bytes: the response R[127:0] as four little-endian words, high word first. VERIFIED-FW by the CSD parser 0x400C_F2E8 (CSD_STRUCTURE = byte 3 bits 7-6, C_SIZE = (b5 & 3) << 10 \| b4 << 2 \| b11 >> 6, ...) and the RCA read after CMD3 (`[+0x20] & 0xFF << 8 \| [+0x24] >> 24`) |
//!
//! Commands the BIOS sends: CMD0, CMD8 (0x1AA), CMD55 + ACMD41 (HCS when CMD8
//! answered, until the busy bit sets), CMD2, CMD3, CMD9, CMD7, CMD55 + ACMD6,
//! CMD17 (read 512 bytes), CMD24 (write 512 bytes).
//!
//! Model: commands complete at once (timing UNKNOWN) and the card is never
//! busy. The card is a 2 GiB standard-capacity SD card that starts freshly
//! formatted (fat.rs; 0.6 never formats it) and stores written sectors. Size:
//! HYPOTHESIS. The real card's size is not known, but it must be over 512
//! MiB: B5 formats the SD volume with 0 root directory entries (0x400C_E920),
//! valid only for FAT32, which it picks above 512 MiB (0x400D_295C). On a 128
//! MiB card B5 made a FAT16 volume with no root directory and could not create
//! `b:\Leapster`. Response CRCs are
//! computed (the BIOS does not check them). The Leapster 2 0.6/B5 base system
//! needs the card: with an empty slot it stops at "SD Card Error: Not
//! Inserted. Please call Customer Service." The CIP3 board has NAND instead
//! (nand.rs) and no SD driver.

use ribbit_memory::Bus;
use std::collections::{BTreeMap, BTreeSet};

pub const BASE: u32 = 0x0180_00E0;
pub const END: u32 = 0x0180_00F8;
pub const INPUTS: u32 = 0x0180_00EC;
/// 0x0180_00EC bit 6: 1 = no card in the slot.
pub const CARD_ABSENT: u32 = 1 << 6;
/// 0x0180_00EC bit 1: the DAT0 line (0 = the card is busy).
pub const DAT0: u32 = 1 << 1;
pub const HOST_START: u32 = 0x0180_E400;
pub const HOST_DESCRIPTOR: u32 = 0x0180_E404;
pub const HOST_MODE: u32 = 0x0180_E408;

const TYPE_READ: u8 = 1 << 0;
const TYPE_WRITE: u8 = 1 << 1;
const TYPE_LONG: u8 = 1 << 2;
const TYPE_SHORT: u8 = 1 << 3;
const TYPE_WIDE: u8 = 1 << 6;
const WRITE_CRC_OK: u16 = 0x2000;

pub const SECTOR: usize = 512;
/// 2 GiB, the largest standard-capacity card (size HYPOTHESIS; see above).
pub const CARD_SECTORS: u32 = 0x40_0000;

/// SD card states (card status bits 12:9).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CardState {
    #[default]
    Idle = 0,
    Ready = 1,
    Ident = 2,
    Stby = 3,
    Tran = 4,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SdCard {
    /// Written sectors (index -> 512 bytes); others read as zeros.
    pub sectors: BTreeMap<u32, Vec<u8>>,
    pub state: CardState,
    pub rca: u16,
    app_cmd: bool,
    pub wide: bool,
    /// A high-capacity (SDHC) card: CCS in the OCR, CSD version 2.0 and
    /// block addressing. Off: a standard-capacity card (byte addressing).
    /// Set from the environment variable RIBBIT_SDHC=1 (an experiment); not
    /// saved, so save states keep their format and load as standard capacity.
    #[serde(skip)]
    pub hc: bool,
    /// Set when a sector was written (for saving; not saved).
    #[serde(skip)]
    pub dirty: bool,
    /// The sectors written since `synced` was set (for saving; not saved).
    #[serde(skip)]
    pub changed: BTreeSet<u32>,
    /// Set by the save code when its file holds `sectors` apart from
    /// `changed`, so it can add just those. A card from a save state starts
    /// unsynced (not saved).
    #[serde(skip)]
    pub synced: bool,
}

impl SdCard {
    /// CID: manufacturer 0, OEM "RB", name "LEAP2", revision 1.0, serial 1,
    /// date 2008-01 (all invented, UNVERIFIED).
    fn cid() -> u128 {
        let mut c: u128 = 0;
        c |= 0x00 << 120;
        c |= (u16::from_be_bytes(*b"RB") as u128) << 104;
        c |= (u64::from_be_bytes([0, 0, 0, b'L', b'E', b'A', b'P', b'2']) as u128 & 0xFF_FFFF_FFFF) << 64;
        c |= 0x10 << 56;
        c |= 1 << 24;
        c |= (0x08 << 4 | 1) << 8;
        with_crc(c)
    }

    /// CSD version 1.0 for 2 GiB, as a 2 GB card's: READ_BL_LEN 10 (1024-byte
    /// blocks; transfers stay 512 bytes, as READ_BL_PARTIAL allows), C_SIZE
    /// 4095, C_SIZE_MULT 7. The BIOS's parser (0.6 0x400C_F2E8) doubles the
    /// sector count for READ_BL_LEN 10.
    fn csd() -> u128 {
        let mut c: u128 = 0;
        c |= 0x0E << 112; // TAAC
        c |= 0x32 << 96; // TRAN_SPEED 25 MHz
        c |= 0x5B5 << 84; // CCC
        c |= 10 << 80; // READ_BL_LEN
        c |= 1 << 79; // READ_BL_PARTIAL
        c |= 4095 << 62; // C_SIZE
        c |= 7 << 47; // C_SIZE_MULT
        c |= 1 << 46; // ERASE_BLK_EN
        c |= 0x7F << 39; // SECTOR_SIZE
        c |= 10 << 22; // WRITE_BL_LEN
        with_crc(c)
    }

    /// CSD version 2.0 (SDHC): C_SIZE in 512 KiB units.
    fn csd_hc() -> u128 {
        let mut c: u128 = 0;
        c |= 1 << 126; // CSD_STRUCTURE 1
        c |= 0x0E << 112; // TAAC
        c |= 0x32 << 96; // TRAN_SPEED 25 MHz
        c |= 0x5B5 << 84; // CCC
        c |= 9 << 80; // READ_BL_LEN
        c |= ((CARD_SECTORS / 1024 - 1) as u128) << 48; // C_SIZE
        c |= 1 << 46; // ERASE_BLK_EN
        c |= 0x7F << 39; // SECTOR_SIZE
        c |= 9 << 22; // WRITE_BL_LEN
        with_crc(c)
    }

    /// The same card after a power cycle: its data, nothing else.
    pub fn powered_on(self) -> Self {
        Self { sectors: self.sectors, dirty: self.dirty, changed: self.changed, synced: self.synced, hc: self.hc, ..Default::default() }
    }

    /// The written sectors as a save file (sparse.rs).
    pub fn image(&self) -> Vec<u8> {
        super::sparse::encode(&self.sectors, SECTOR)
    }

    /// Replaces the card's contents with a save file from `image`; false
    /// (card unchanged) if the file is not one.
    pub fn load_image(&mut self, bytes: &[u8]) -> bool {
        match super::sparse::decode(bytes, SECTOR) {
            Some(mut s) => {
                s.retain(|&n, _| n < CARD_SECTORS);
                self.sectors = s;
                self.changed.clear();
                self.synced = false;
                true
            }
            None => false,
        }
    }

    fn status(&self) -> u32 {
        ((self.state as u32) << 9) | (1 << 8) | ((self.app_cmd as u32) << 5)
    }

    /// Runs one command; the response as R[127:0] (48-bit responses in the
    /// low bits), or None if the card does not answer.
    fn command(&mut self, cmd: u8, arg: u32) -> Option<u128> {
        let app = std::mem::take(&mut self.app_cmd);
        let r1 = |s: &Self| Some(short(cmd, s.status()));
        match (app, cmd) {
            (_, 0) => {
                *self = std::mem::take(self).powered_on();
                None
            }
            (_, 8) => Some(short(8, arg & 0xFFF)),
            (_, 55) => {
                self.app_cmd = true;
                r1(self)
            }
            (true, 41) => {
                // Ready at once: busy bit 31 set, 2.7-3.6 V, standard capacity.
                if self.state == CardState::Idle {
                    self.state = CardState::Ready;
                }
                Some(short(0x3F, 0x80FF_8000 | (self.hc as u32) << 30) | 0xFF)
            }
            (true, 6) => {
                self.wide = arg & 3 == 2;
                r1(self)
            }
            (_, 2) => {
                self.state = CardState::Ident;
                Some(Self::cid())
            }
            (_, 3) => {
                self.state = CardState::Stby;
                self.rca = 0x1234;
                let s = self.status();
                Some(short(3, (self.rca as u32) << 16 | (s >> 6 & 0x2000) | (s & 0x1FFF)))
            }
            (_, 9) => Some(if self.hc { Self::csd_hc() } else { Self::csd() }),
            (_, 7) => {
                // RCA 0 deselects every card (SD specification).
                let me = arg >> 16 != 0 && (arg >> 16) as u16 == self.rca;
                let before = self.status();
                self.state = if me { CardState::Tran } else { CardState::Stby };
                me.then(|| short(7, before))
            }
            (_, 13) => r1(self),
            (_, 16) | (_, 17) | (_, 24) => r1(self),
            _ => None,
        }
    }

    fn sector(&self, n: u32) -> Vec<u8> {
        self.sectors.get(&n).cloned().unwrap_or_else(|| vec![0; SECTOR])
    }
}

/// A 48-bit response: index, 32-bit payload, CRC7, end bit.
fn short(index: u8, payload: u32) -> u128 {
    let body = (index as u64 & 0x3F) << 32 | payload as u64;
    let crc = crc7(&body.to_be_bytes()[3..8]);
    ((body << 8) | (crc as u64) << 1 | 1) as u128
}

/// CID/CSD with their CRC7 in bits 7:1 and the end bit.
fn with_crc(v: u128) -> u128 {
    let crc = crc7(&v.to_be_bytes()[..15]);
    (v & !0xFF) | (crc as u128) << 1 | 1
}

fn crc7(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in bytes {
        for i in (0..8).rev() {
            let bit = (b >> i) & 1;
            let top = (crc >> 6) & 1;
            crc = (crc << 1) & 0x7F;
            if bit ^ top != 0 {
                crc ^= 0x09;
            }
        }
    }
    crc
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SdSlot {
    /// Latches 0x0180_00E0..=0x0180_00F8 (the input register's entry unused).
    pub latches: [u32; 7],
    pub card: Option<SdCard>,
    pub descriptor: u32,
    pub mode: u32,
    /// A descriptor to run after the current instruction (needs memory).
    pub pending: Option<u32>,
    pub commands: u64,
    /// Research: the first commands as (command, argument, status) (not saved).
    #[serde(skip)]
    pub log: Vec<(u8, u32, u16)>,
}

fn index(addr: u32) -> Option<usize> {
    ((BASE..=END).contains(&addr) && addr.is_multiple_of(4)).then(|| ((addr - BASE) / 4) as usize)
}

impl SdSlot {
    /// A slot with a freshly formatted card in it (fat.rs).
    pub fn with_card() -> Self {
        let sectors = super::fat::format_fat32(CARD_SECTORS).into_iter().collect();
        let hc = std::env::var("RIBBIT_SDHC").is_ok_and(|v| v == "1");
        Self { card: Some(SdCard { sectors, hc, ..Default::default() }), ..Default::default() }
    }

    pub fn inputs(&self) -> u32 {
        if self.card.is_some() { DAT0 } else { CARD_ABSENT | DAT0 }
    }

    pub fn read(&self, addr: u32) -> Option<u32> {
        match addr {
            INPUTS => Some(self.inputs()),
            HOST_START => Some(0),
            HOST_DESCRIPTOR => Some(self.descriptor),
            HOST_MODE => Some(self.mode),
            _ => index(addr).map(|i| self.latches[i]),
        }
    }

    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        match addr {
            INPUTS => return false,
            HOST_START => {
                if value & 1 != 0 {
                    self.pending = Some(self.descriptor);
                }
            }
            HOST_DESCRIPTOR => self.descriptor = value,
            HOST_MODE => self.mode = value,
            _ => match index(addr) {
                Some(i) => self.latches[i] = value,
                None => return false,
            },
        }
        true
    }

    /// Runs the descriptor at `d` (the controller's DMA).
    pub fn run(&mut self, d: u32, bus: &mut impl Bus) {
        self.commands += 1;
        let kind = bus.read8(d + 2);
        let token = bus.read16(d + 4);
        let cmd = (token >> 8) as u8 & 0x3F;
        let arg = (token as u32 & 0xFF) << 24 | bus.read32(d + 8) >> 8;
        let data = kind & (TYPE_READ | TYPE_WRITE) != 0;
        let response_at = d + if data { 0x20 } else { 0x18 };
        let mut status = 0u16;
        let answer = self.card.as_mut().and_then(|c| c.command(cmd, arg));
        match answer {
            Some(r) if kind & (TYPE_SHORT | TYPE_LONG) != 0 => {
                for w in 0..4 {
                    bus.write32(response_at + 4 * w, (r >> (96 - 32 * w)) as u32);
                }
            }
            // No response: the response area keeps what the driver put there
            // (all ones) and status bit 0 stays clear. HYPOTHESIS from the
            // BIOS's own use: after every access it deselects the card with
            // CMD7 and RCA 0 (0x400C_F12C), which a card does not answer (SD
            // specification), asking for a response; with bit 0 set for it
            // the driver gives the card up ("Cmd=7 Got Nonzero DMA Status
            // bits 0001", "GetToStandbyState Failed") and nothing after the
            // first sector is read. So bit 0 is some other error (response
            // CRC?), not a timeout.
            _ => {}
        }
        if data && answer.is_some() {
            let len = (bus.read16(d + 0x0C) as usize).min(SECTOR);
            let buf = bus.read32(d + 0x10);
            let card = self.card.as_mut().unwrap();
            // Byte address on a standard-capacity card, block number on SDHC.
            let n = if card.hc { arg } else { arg / SECTOR as u32 };
            if kind & TYPE_WRITE != 0 {
                let mut s = card.sector(n);
                for (i, b) in s.iter_mut().enumerate().take(len) {
                    *b = bus.read8(buf + i as u32);
                }
                if n < CARD_SECTORS {
                    card.sectors.insert(n, s);
                    card.changed.insert(n);
                    card.dirty = true;
                }
                status |= WRITE_CRC_OK;
            } else {
                let s = if n < CARD_SECTORS { card.sector(n) } else { vec![0; SECTOR] };
                for (i, &b) in s.iter().enumerate().take(len) {
                    bus.write8(buf + i as u32, b);
                }
                status |= if kind & TYPE_WIDE != 0 { 0xF0 } else { 0x10 };
            }
        }
        bus.write16(d, status);
        bus.write8(d + 3, 0);
        if self.log.len() < 1 << 16 {
            self.log.push((cmd, arg, status));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mem(Vec<u8>);
    impl Bus for Mem {
        fn read8(&mut self, a: u32) -> u8 {
            self.0[a as usize]
        }
        fn read16(&mut self, a: u32) -> u16 {
            u16::from_le_bytes([self.read8(a), self.read8(a + 1)])
        }
        fn read32(&mut self, a: u32) -> u32 {
            self.read16(a) as u32 | (self.read16(a + 2) as u32) << 16
        }
        fn write8(&mut self, a: u32, v: u8) {
            self.0[a as usize] = v;
        }
        fn write16(&mut self, a: u32, v: u16) {
            self.write8(a, v as u8);
            self.write8(a + 1, (v >> 8) as u8);
        }
        fn write32(&mut self, a: u32, v: u32) {
            self.write16(a, v as u16);
            self.write16(a + 2, (v >> 16) as u16);
        }
    }

    /// Builds a descriptor at 0x100 as the BIOS does (0x400C_E970 / EA84).
    fn command(slot: &mut SdSlot, mem: &mut Mem, cmd: u8, kind: u8, arg: u32) -> u16 {
        let d = 0x100;
        mem.write16(d, 0xFFFF);
        mem.write8(d + 2, kind);
        mem.write8(d + 3, 0x80);
        mem.write16(d + 4, (0x40 | cmd as u16) << 8 | (arg >> 24) as u16);
        mem.write32(d + 8, arg << 8);
        if kind & (TYPE_READ | TYPE_WRITE) != 0 {
            mem.write16(d + 0x0C, 0x200);
            mem.write32(d + 0x10, 0x400);
        }
        slot.write(HOST_DESCRIPTOR, d);
        slot.write(HOST_START, 1);
        let p = slot.pending.take().unwrap();
        slot.run(p, mem);
        assert_eq!(mem.read8(d + 3), 0, "active bit cleared");
        mem.read16(d)
    }

    fn response(mem: &mut Mem, data: bool) -> [u8; 16] {
        let at = if data { 0x120 } else { 0x118 };
        std::array::from_fn(|i| mem.read8(at + i as u32))
    }

    #[test]
    fn empty_slot_reports_no_card_and_dat0_idle() {
        let s = SdSlot::default();
        let ec = s.read(INPUTS).unwrap();
        // The BIOS's card-detect routine (0x400C_ED54) returns 0 = no card.
        assert_eq!((ec >> 6 & 1) ^ 1, 0);
        assert_ne!(ec & DAT0, 0);
        assert_eq!((SdSlot::with_card().read(INPUTS).unwrap() >> 6 & 1) ^ 1, 1);
    }

    #[test]
    fn identification_as_the_bios_does_it() {
        let (mut s, mut m) = (SdSlot::with_card(), Mem(vec![0; 0x1000]));
        assert_eq!(command(&mut s, &mut m, 0, 0, 0) & 1, 0);
        assert_eq!(command(&mut s, &mut m, 8, 8, 0x1AA) & 1, 0);
        let r = response(&mut m, false);
        // The BIOS checks [+0x26] & 0xF == 1 and [+0x25] == 0xAA (0x400C_EE6E).
        assert_eq!((r[0xE] & 0xF, r[0xD]), (1, 0xAA));
        command(&mut s, &mut m, 55, 8, 0);
        command(&mut s, &mut m, 41, 8, 0x4010_0000);
        assert_ne!(m.read32(0x120) & 0x80, 0, "ACMD41 busy bit (OCR bit 31) at +0x20 bit 7");
        command(&mut s, &mut m, 2, 4, 0);
        command(&mut s, &mut m, 3, 8, 0);
        // RCA as the BIOS reads it (0x400C_EF86).
        let rca = (m.read32(0x120) & 0xFF) << 8 | m.read32(0x124) >> 24;
        assert_eq!(rca, 0x1234);
        command(&mut s, &mut m, 9, 4, rca << 16);
        let csd = response(&mut m, false);
        // The BIOS's parser (0x400C_F2E8) on the copied bytes.
        assert_eq!(csd[3] >> 6, 0, "CSD version 1");
        let c_size = ((csd[5] & 3) as u32) << 10 | (csd[4] as u32) << 2 | (csd[11] >> 6) as u32;
        let mult = ((csd[10] & 3) as u32) << 1 | (csd[9] >> 7) as u32;
        assert_eq!(csd[6] & 0xF, 10, "READ_BL_LEN 1024");
        assert_eq!((c_size + 1) * (4 << mult) * 2, CARD_SECTORS);
        assert_eq!(command(&mut s, &mut m, 7, 8, rca << 16) & 1, 0);
        assert_eq!(s.card.as_ref().unwrap().state, CardState::Tran);
    }

    #[test]
    fn sectors_write_and_read_back() {
        let (mut s, mut m) = (SdSlot::with_card(), Mem(vec![0; 0x1000]));
        for i in 0..0x200 {
            m.write8(0x400 + i, i as u8);
        }
        let st = command(&mut s, &mut m, 24, 0x4A, 3 * 512);
        assert_eq!(st & 0xB000, 0x2000, "positive CRC token");
        m.0[0x400..0x600].fill(0);
        let st = command(&mut s, &mut m, 17, 0x49, 3 * 512);
        assert_eq!(st & 0xF0, 0xF0, "all four data lines good");
        assert_eq!(m.read8(0x400 + 0x1FF), 0xFF);
        command(&mut s, &mut m, 17, 0x09, 1000 * 512);
        assert_eq!(m.read8(0x401), 0, "unwritten sectors read zeros");
        command(&mut s, &mut m, 17, 0x09, 0);
        assert_eq!(m.read16(0x400 + 510), 0xAA55, "the formatted card's partition table");
    }

    #[test]
    fn unanswered_commands_leave_the_response_area() {
        let (mut s, mut m) = (SdSlot::with_card(), Mem(vec![0; 0x1000]));
        m.0[0x118..0x128].fill(0xFF);
        // CMD7 with RCA 0 deselects; the card stays silent (BIOS 0x400C_F12C).
        assert_eq!(command(&mut s, &mut m, 7, 8, 0), 0);
        assert_eq!(m.read32(0x124), 0xFFFF_FFFF);
    }

    #[test]
    fn crc7_of_cmd0() {
        // The well-known CMD0 token 40 00 00 00 00 has CRC7 0x4A (byte 0x95).
        assert_eq!(crc7(&[0x40, 0, 0, 0, 0]), 0x4A);
    }
}
