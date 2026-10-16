//! Physical address map of the original Leapster.
//!
//! Source: MAME `leapster.cpp` (`leapster_map`) cross-checked against the
//! LeapFrog-Tools README (BIOS at 0x4000_0000, CART at 0x8000_0000,
//! SDRAM at 0x3C00_0000) and pointer values inside a retail cartridge
//! (`0x80xx_xxxx`). See docs/hardware-research.md §Memory map.

/// Firmware ("BaseROM") window. Also visible at `BIOS_MIRROR`.
pub const BIOS_BASE: u32 = 0x0000_0000;
/// MAME maps the BIOS with `.mirror(0x4000_0000)`; the default interrupt
/// vector base is 0x4000_0000 and BIOS code executes from there.
pub const BIOS_MIRROR: u32 = 0x4000_0000;
/// Size of the BIOS window (largest known original-Leapster BIOS is 8 MiB).
pub const BIOS_WINDOW: u32 = 0x0080_0000;

/// On-chip peripheral registers (interrupt controller, ADC, LCD, DMA,
/// timers, UART, audio, GPIO). Decoded by ribbit-core, not by this crate.
pub const MMIO_BASE: u32 = 0x0180_0000;
pub const MMIO_SIZE: u32 = 0x0001_0000;

/// Video RAM; also used as stack before the RTOS starts (MAME comment).
pub const VRAM_BASE: u32 = 0x0300_0000;
pub const VRAM_SIZE: u32 = 0x0001_0000;
/// The v1.5 BIOS clear-screen routine writes past VRAM; MAME discards
/// these writes. HYPOTHESIS: real hardware behaviour unknown.
pub const VRAM_OVERRUN_END: u32 = 0x0303_0000;

/// Main SDRAM. 2 MiB on the original Leapster (ISSI IS42S16100 on the PCB).
pub const RAM_BASE: u32 = 0x3C00_0000;
pub const RAM_SIZE: u32 = 0x0020_0000;
/// Leapster 2 SDRAM: 16 MiB. VERIFIED-FW: its BIOS (0.6, 0x4004_0624) tests
/// RAM by writing a pattern at every 1 MiB from 0x3C00_0000 to 0x3D00_0000
/// and reports error 2 (a repeating double beep) if any reads back wrong.
pub const RAM_SIZE_LEAPSTER2: u32 = 0x0100_0000;
/// Largest RAM window (the decode window for any model).
pub const RAM_WINDOW: u32 = RAM_SIZE_LEAPSTER2;

/// Leapster 2: on-chip RAM used by the USB driver. VERIFIED-FW (BIOS 0.6,
/// 0x400B_E964): the driver creates its MQX memory pool with the constants
/// `1 << 26` and `1 << 13`, i.e. 8 KiB at 0x0400_0000, and allocates its
/// endpoint buffers there. What lies beyond 8 KiB, and whether it mirrors,
/// is UNKNOWN (only the BIOS's use is modelled).
pub const SRAM_BASE: u32 = 0x0400_0000;
pub const SRAM_SIZE_LEAPSTER2: u32 = 0x2000;

/// Cartridge ROM window. Retail carts are 4, 8 or 16 MiB.
pub const CART_BASE: u32 = 0x8000_0000;
pub const CART_WINDOW: u32 = 0x0100_0000;
