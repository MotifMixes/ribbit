//! The DMA controller: 8 channels, each with a command queue (kernel
//! drivers/lf1000/dma/dma.c, include/asm-arm/arch-lf1000/dma.h).
//!
//! Per channel, 0x80 bytes apart from 0xC000_0000: source 0x0, destination
//! 0x4, length - 1 (16 bits) 0x8, request ID (16 bits) 0xA, mode 0xC. The
//! same four at +0x10 queue a command while the channel is busy (the
//! driver writes them there when CMDSPACE shows room); CMDWAIT 0x20,
//! CMDSTOP 0x24 (empties the queue), CMDBUSY 0x28, CMDSPACE 0x2C (free
//! queue slots, bits 0-2). Mode: RUN bit 19 starts the command, INTENB 18,
//! INTPEND 17 (write 1 to clear), BUSY 16; the rest are the transfer flags.
//!
//! Transfers to or from a peripheral run at that peripheral's pace: PCM
//! out (request 24, the I2S audio) takes 4 bytes per sample at the
//! sample rate. Other transfers finish at once and move no data (not
//! modelled yet: no driver seen so far needs them).

pub const BASE: u32 = 0xC000_0000;
pub const END: u32 = BASE + CHANNELS as u32 * 0x80;
pub const IRQ: u32 = 3;
const CHANNELS: usize = 8;
/// Queue slots per channel (GUESS: CMDSPACE has 3 bits; the driver takes
/// what it reads after a stop as the queue's size).
const QUEUE: usize = 7;

const MODE: u32 = 0x0C;
const QUEUED: u32 = 0x10;
const CMDSTOP: u32 = 0x24;
const CMDBUSY: u32 = 0x28;
const CMDSPACE: u32 = 0x2C;

const RUN: u32 = 1 << 19;
const INTENB: u32 = 1 << 18;
const INTPEND: u32 = 1 << 17;
const BUSY: u32 = 1 << 16;
const DST_IO: u32 = 1 << 10;
const SRC_IO: u32 = 1 << 2;
#[cfg(test)]
const DST_NOINC: u32 = 1 << 12;

/// Request IDs (dma.h).
pub const PCMOUT: u32 = 24;

/// A command: source, destination, length - 1 | request ID << 16, mode.
type Command = [u32; 4];

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Channel {
    /// The registers at 0x00-0x0C (the running or last command).
    current: Command,
    /// The registers at 0x10-0x1C, waiting for their mode write.
    staged: Command,
    queue: std::collections::VecDeque<Command>,
    busy: bool,
    /// Bytes of the running transfer still to go, and the cycle-rate
    /// remainder carried between ticks.
    left: u64,
    acc: u64,
    intenb: bool,
    intpend: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Dma {
    ch: [Channel; CHANNELS],
    /// The PCM-out sample rate (Hz), set by the audio model; 32000 until
    /// then (what the Brio audio driver asks for).
    pub pcm_rate: u64,
    /// Audio sent to PCM out (16-bit stereo, little-endian), oldest first,
    /// at most a second of it.
    pub pcm: std::collections::VecDeque<u8>,
}

impl Default for Dma {
    fn default() -> Self {
        Self { ch: Default::default(), pcm_rate: 32000, pcm: Default::default() }
    }
}

fn reqid(c: &Command) -> u32 {
    c[2] >> 16
}

fn len(c: &Command) -> u64 {
    (c[2] & 0xFFFF) as u64 + 1
}

impl Dma {
    pub fn read(&self, addr: u32) -> u32 {
        let (c, off) = ((addr - BASE) / 0x80, (addr - BASE) % 0x80);
        let ch = &self.ch[c as usize];
        match off {
            0x00..=0x08 => ch.current[off as usize / 4],
            MODE => {
                let mut v = ch.current[3] & !(RUN | INTENB | INTPEND | BUSY);
                if ch.busy {
                    v |= RUN | BUSY;
                }
                v | if ch.intenb { INTENB } else { 0 } | if ch.intpend { INTPEND } else { 0 }
            }
            0x10..=0x1C => ch.staged[(off - QUEUED) as usize / 4],
            CMDBUSY => 0,
            CMDSPACE => (QUEUE - ch.queue.len()) as u32,
            _ => 0,
        }
    }

    pub fn write(&mut self, addr: u32, v: u32, sdram: &[u8]) {
        let (c, off) = ((addr - BASE) / 0x80, (addr - BASE) % 0x80);
        let ch = &mut self.ch[c as usize];
        match off {
            0x00..=0x08 => ch.current[off as usize / 4] = v,
            0x10..=0x18 => ch.staged[(off - QUEUED) as usize / 4] = v,
            MODE | 0x1C => {
                ch.intenb = v & INTENB != 0;
                if v & INTPEND != 0 {
                    ch.intpend = false;
                }
                if off == MODE {
                    ch.current[3] = v & !(INTPEND | BUSY);
                    if v & RUN != 0 && !ch.busy {
                        let cmd = ch.current;
                        self.start(c as usize, cmd, sdram);
                    }
                } else if v & RUN != 0 {
                    let mut cmd = ch.staged;
                    cmd[3] = v;
                    if ch.busy {
                        if ch.queue.len() < QUEUE {
                            ch.queue.push_back(cmd);
                        }
                    } else {
                        self.start(c as usize, cmd, sdram);
                    }
                }
            }
            CMDSTOP => {
                ch.queue.clear();
                ch.busy = false;
            }
            _ => {}
        }
    }

    fn paced(cmd: &Command) -> bool {
        cmd[3] & (DST_IO | SRC_IO) != 0 && reqid(cmd) == PCMOUT
    }

    fn start(&mut self, c: usize, cmd: Command, sdram: &[u8]) {
        let ch = &mut self.ch[c];
        ch.current = cmd;
        ch.current[3] &= !(INTPEND | BUSY);
        ch.busy = true;
        ch.left = len(&cmd);
        ch.acc = 0;
        if !Self::paced(&cmd) {
            self.finish(c, sdram);
        }
    }

    /// The running command is done: PCM sent out is kept, then the next
    /// queued command starts.
    fn finish(&mut self, c: usize, sdram: &[u8]) {
        let cmd = self.ch[c].current;
        if Self::paced(&cmd) && cmd[3] & SRC_IO == 0 {
            let src = cmd[0] as usize;
            if let Some(bytes) = sdram.get(src..src + len(&cmd) as usize) {
                self.pcm.extend(bytes);
                let max = self.pcm_rate as usize * 4;
                if self.pcm.len() > max {
                    self.pcm.drain(..self.pcm.len() - max);
                }
            }
        }
        let ch = &mut self.ch[c];
        ch.busy = false;
        ch.left = 0;
        ch.intpend = true;
        if let Some(next) = ch.queue.pop_front() {
            self.start(c, next, sdram);
        }
    }

    /// Advances paced transfers by `cycles` at `cpu_hz`.
    pub fn tick(&mut self, cycles: u64, cpu_hz: u64, sdram: &[u8]) {
        let rate = self.pcm_rate * 4;
        for c in 0..CHANNELS {
            let ch = &mut self.ch[c];
            if !ch.busy {
                continue;
            }
            ch.acc += cycles * rate;
            let done = ch.acc / cpu_hz;
            ch.acc %= cpu_hz;
            if done >= ch.left {
                self.finish(c, sdram);
            } else {
                ch.left -= done;
            }
        }
    }

    pub fn irq(&self) -> bool {
        self.ch.iter().any(|c| c.intenb && c.intpend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CH: u32 = BASE + 0x80; // channel 1

    fn launch(d: &mut Dma, off: u32, src: u32, n: u32, ram: &[u8]) {
        let flags = DST_IO | 1 << 8 | DST_NOINC; // to PCM out, 16-bit
        d.write(CH + off, src, ram);
        d.write(CH + off + 4, 0xC000_D800, ram);
        d.write(CH + off + 8, (n - 1) | PCMOUT << 16, ram);
        d.write(CH + off + 0xC, flags | INTENB | RUN, ram);
    }

    #[test]
    fn pcm_out_takes_its_time_and_queues() {
        let mut ram = vec![0u8; 0x4000];
        ram[0x1000..0x1004].copy_from_slice(&[1, 2, 3, 4]);
        let mut d = Dma::default();
        assert_eq!(d.read(CH + CMDSPACE), QUEUE as u32);
        launch(&mut d, 0, 0x1000, 4096, &ram);
        assert!(d.read(CH + MODE) & BUSY != 0);
        // Busy: the driver queues the next one at +0x10.
        launch(&mut d, QUEUED, 0x2000, 4096, &ram);
        assert_eq!(d.read(CH + CMDSPACE), QUEUE as u32 - 1);
        // 4096 bytes at 32000 Hz * 4: 32 ms; at 100 MHz, 3.2M cycles.
        d.tick(3_000_000, 100_000_000, &ram);
        assert!(!d.irq());
        d.tick(300_000, 100_000_000, &ram);
        assert!(d.irq(), "first done");
        assert!(d.read(CH + MODE) & BUSY != 0, "the queued one runs");
        assert_eq!(d.read(CH), 0x2000);
        assert_eq!(d.pcm.iter().take(4).copied().collect::<Vec<_>>(), [1, 2, 3, 4]);
        // Clearing INTPEND as lf1000_dma_int_clr does (read, set bit 17, write).
        let m = d.read(CH + MODE);
        d.write(CH + MODE, m | INTPEND, &ram);
        assert!(!d.irq());
        assert!(d.read(CH + MODE) & BUSY != 0, "still running");
        d.write(CH + CMDSTOP, 1, &ram);
        assert_eq!(d.read(CH + MODE) & BUSY, 0);
    }
}
