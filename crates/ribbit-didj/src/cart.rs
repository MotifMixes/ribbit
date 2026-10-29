//! A Didj cartridge: small-page NAND (512-byte pages with 16 bytes of spare
//! area, 32 pages per 16 KiB erase block) on the NAND controller's second
//! bank (NFCONTROL bit 0, `lf1000_select_cart`).
//!
//! Dumps are the raw flash, data and spare area page by page: 69206016 bytes
//! for a 64 MB chip (4096 blocks). They hold UBI with one volume,
//! "ubi_Cartridge" (FAT32), which the kernel attaches as the "Cartridge" MTD
//! and Brio mounts at /Cart.
//!
//! Small-page chips have no read-confirm command: 0x00, 0x01 and 0x50 point
//! at the first half, the second half and the spare area, and the page is
//! ready once its address is in (one column byte, three row bytes; Linux
//! nand_base.c `nand_command`). The ID is Toshiba's 512 Mbit OTP part (nand
//! lf1000.c OTP_DEVID_512MBIT, 0x98 0x76); what other cartridges answer is
//! UNKNOWN, so smaller or larger dumps get the ID of their size.

pub const PAGE: usize = 512;
pub const OOB: usize = 16;
pub const PAGE_RAW: usize = PAGE + OOB;
pub const PAGES_PER_BLOCK: usize = 32;

/// The cartridge type lightning-boot reads from GPIO port B pins 2-5 into
/// the scratch register (nand.h): production carts are OTP with ECC off.
/// GUESS: retail cartridges are production ones.
pub const PRODUCTION: u32 = 0x7;

pub struct Cart {
    /// The raw flash, PAGE_RAW bytes per page.
    flash: Vec<u8>,
    /// Command state of the chip.
    cmd: u8,
    /// Where 0x00 / 0x01 / 0x50 point (0, 256, 512).
    area: usize,
    addr: Vec<u8>,
    /// The page being read and the byte reached in it.
    page: usize,
    pos: usize,
    /// Bytes written since 0x80.
    program: Vec<u8>,
    id_pos: usize,
    /// Pages written or blocks erased since the dump was loaded.
    pub changed: bool,
}

impl Cart {
    /// A dump: whole blocks of raw pages.
    pub fn new(flash: Vec<u8>) -> Result<Self, String> {
        let block = PAGE_RAW * PAGES_PER_BLOCK;
        if flash.is_empty() || flash.len() % block != 0 {
            return Err(format!("{} bytes is not a Didj cartridge dump (blocks of {block} bytes)", flash.len()));
        }
        Ok(Self { flash, cmd: 0, area: 0, addr: Vec::new(), page: 0, pos: 0, program: Vec::new(), id_pos: 0, changed: false })
    }

    /// True for a file of a cartridge dump's size with UBI at its start.
    pub fn looks_like(data: &[u8]) -> bool {
        data.len() % (PAGE_RAW * PAGES_PER_BLOCK) == 0 && data.starts_with(b"UBI#")
    }

    pub fn raw(&self) -> &[u8] {
        &self.flash
    }

    fn pages(&self) -> usize {
        self.flash.len() / PAGE_RAW
    }

    fn row(&self) -> usize {
        self.addr.iter().skip(1).take(3).enumerate().fold(0, |a, (i, &b)| a | (b as usize) << (8 * i)) % self.pages()
    }

    pub fn command(&mut self, cmd: u8) {
        match cmd {
            0x00 | 0x01 | 0x50 => {
                self.area = match cmd {
                    0x00 => 0,
                    0x01 => 256,
                    _ => PAGE,
                };
                self.addr.clear();
                self.cmd = 0x00;
            }
            0x80 | 0x60 | 0x90 => {
                self.addr.clear();
                self.program.clear();
                self.id_pos = 0;
                self.cmd = cmd;
            }
            0x10 => {
                // Program: bits only go from 1 to 0.
                let start = self.row() * PAGE_RAW + self.area + self.addr.first().copied().unwrap_or(0) as usize;
                let end = (self.row() + 1) * PAGE_RAW;
                for (x, b) in self.flash[start.min(end)..end].iter_mut().zip(&self.program) {
                    *x &= b;
                }
                self.changed = true;
                self.area = 0;
                self.cmd = 0;
            }
            0xD0 => {
                // Erase: three row bytes.
                let page = self.addr.iter().take(3).enumerate().fold(0, |a, (i, &b)| a | (b as usize) << (8 * i)) % self.pages();
                let block = page / PAGES_PER_BLOCK * PAGES_PER_BLOCK * PAGE_RAW;
                self.flash[block..block + PAGES_PER_BLOCK * PAGE_RAW].fill(0xFF);
                self.changed = true;
                self.cmd = 0;
            }
            0xFF => {
                self.area = 0;
                self.cmd = 0;
            }
            _ => self.cmd = cmd,
        }
    }

    pub fn address(&mut self, b: u8) {
        self.addr.push(b);
        if self.cmd == 0x00 && self.addr.len() == 4 {
            self.page = self.row();
            self.pos = self.area + self.addr[0] as usize;
            // 0x01 points at the second half for one read only.
            if self.area == 256 {
                self.area = 0;
            }
        }
    }

    pub fn read_byte(&mut self) -> u8 {
        match self.cmd {
            0x70 => 0xC0, // ready, not write protected, no error
            0x90 => {
                let id = match self.flash.len() / PAGE_RAW * PAGE >> 20 {
                    16 => 0x73,
                    32 => 0x75,
                    128 => 0x79,
                    _ => 0x76,
                };
                let b = [0x98, id].get(self.id_pos).copied().unwrap_or(0);
                self.id_pos += 1;
                b
            }
            _ => {
                // Reads run on into the next page.
                if self.pos >= PAGE_RAW {
                    self.page = (self.page + 1) % self.pages();
                    self.pos = if self.area == PAGE { PAGE } else { 0 };
                }
                let b = self.flash[self.page * PAGE_RAW + self.pos];
                self.pos += 1;
                b
            }
        }
    }

    pub fn write_byte(&mut self, b: u8) {
        if self.cmd == 0x80 {
            self.program.push(b);
        }
    }
}

/// The data area of a dump (spare areas dropped).
fn data(raw: &[u8]) -> Vec<u8> {
    raw.chunks(PAGE_RAW).flat_map(|p| &p[..PAGE.min(p.len())]).copied().collect()
}

/// The cartridge's UBI volume 0 ("ubi_Cartridge", a FAT32 volume): each
/// erase block's VID header names the logical block it holds; where two
/// hold the same one, the higher sequence number wins (ubi-media.h).
pub fn volume(raw: &[u8]) -> Option<Vec<u8>> {
    let flash = data(raw);
    let peb = PAGE * PAGES_PER_BLOCK;
    let be32 = |b: &[u8], o: usize| u32::from_be_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let mut lebs: std::collections::BTreeMap<usize, (u64, usize)> = Default::default();
    let mut data_offset = 0;
    for (n, block) in flash.chunks_exact(peb).enumerate() {
        if !block.starts_with(b"UBI#") {
            continue;
        }
        let (vid, offset) = (be32(block, 0x10), be32(block, 0x14));
        if vid + 0x40 > peb || offset >= peb || &block[vid..vid + 4] != b"UBI!" || be32(block, vid + 8) != 0 {
            continue;
        }
        data_offset = offset;
        let sqnum = u64::from_be_bytes(block[vid + 40..vid + 48].try_into().unwrap());
        let e = lebs.entry(be32(block, vid + 12)).or_insert((sqnum, n));
        if sqnum >= e.0 {
            *e = (sqnum, n);
        }
    }
    let leb = peb - data_offset;
    let len = (lebs.keys().last()? + 1) * leb;
    let mut out = vec![0xFF; len];
    for (l, (_, n)) in lebs {
        out[l * leb..][..leb].copy_from_slice(&flash[n * peb + data_offset..][..leb]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cart() -> Cart {
        let mut flash = vec![0xFF; PAGE_RAW * PAGES_PER_BLOCK * 4];
        for (i, b) in flash.iter_mut().enumerate() {
            *b = (i / PAGE_RAW) as u8 ^ (i % PAGE_RAW) as u8;
        }
        Cart::new(flash).unwrap()
    }

    fn read(c: &mut Cart, cmd: u8, col: u8, page: usize, n: usize) -> Vec<u8> {
        c.command(cmd);
        for b in [col, page as u8, (page >> 8) as u8, (page >> 16) as u8] {
            c.address(b);
        }
        (0..n).map(|_| c.read_byte()).collect()
    }

    #[test]
    fn read_areas() {
        let mut c = cart();
        assert_eq!(read(&mut c, 0x00, 3, 5, 2), [5 ^ 3, 5 ^ 4]);
        assert_eq!(read(&mut c, 0x01, 0, 5, 1), [5 ^ 0]); // byte 256
        assert_eq!(read(&mut c, 0x50, 2, 7, 1), [7 ^ (514 % 256) as u8 ^ 0]);
        // Into the next page.
        let r = read(&mut c, 0x00, 0, 1, PAGE_RAW + 1);
        assert_eq!(r[PAGE_RAW], 2);
    }

    #[test]
    fn program_and_erase() {
        let mut c = cart();
        c.command(0x60);
        for b in [33, 0, 0] {
            c.address(b);
        }
        c.command(0xD0);
        assert_eq!(read(&mut c, 0x00, 0, 33, 1), [0xFF]);
        c.command(0x00);
        c.command(0x80);
        for b in [10, 33, 0, 0] {
            c.address(b);
        }
        c.write_byte(0x12);
        c.command(0x10);
        assert_eq!(read(&mut c, 0x00, 10, 33, 2), [0x12, 0xFF]);
        assert!(c.changed);
        c.command(0x90);
        c.address(0);
        assert_eq!([c.read_byte(), c.read_byte()], [0x98, 0x76]);
    }
}
