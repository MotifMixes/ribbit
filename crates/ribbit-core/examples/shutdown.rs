//! How long a game's power-button shutdown takes to reach a black screen
//! (Ribbit's "Animate game shutdown"): `shutdown <bios> <cart>...`.
//! Boots each game for 10 s, presses the power button (the frontend's way:
//! NO_POWER_OFF_REQUEST low), and prints when the screen first goes black by
//! the frontend's test, and how light it is at 1, 2, 3 and 6 s.
use ribbit_core::Machine;
use ribbit_debug::Breakpoints;
use ribbit_video::{HEIGHT, WIDTH};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    for cart in &a[2..] {
        let fw = ribbit_core::load_firmware(std::path::Path::new(&a[1])).expect("BIOS");
        let c = ribbit_core::load_cartridge(std::path::Path::new(cart)).expect("cartridge");
        // As the frontend: calibration skipped (the saved one), touch on.
        let mut m = Machine::with_straps(fw, Some(c), 0x63FF_FFFF);
        m.enable_experimental_mame_touch(ribbit_core::io::touch::MameTouchConfig::default());
        let frame_cycles = m.clock.cpu_hz as u64 / 60;
        let mut frame = Box::new([0u32; WIDTH * HEIGHT]);
        let mut before = Vec::new();
        for f in 1..=1800u32 {
            m.run_cycles(frame_cycles, &Breakpoints::default());
            if f % 300 == 0 {
                m.render_frame(&mut frame);
                let light: u64 = frame.iter().map(|p| ((p >> 16) & 0xFF).max((p >> 8) & 0xFF).max(p & 0xFF) as u64).sum();
                before.push(format!("{}s:{:.0}", f / 60, light as f64 / frame.len() as f64));
            }
        }
        println!("  before: {}", before.join(" "));
        m.io.gpio.board_lines &= !ribbit_core::io::gpio::NO_POWER_OFF_REQUEST;
        let mut black_at = None;
        let mut lights = Vec::new();
        for f in 1..=900u32 {
            m.run_cycles(frame_cycles, &Breakpoints::default());
            m.render_frame(&mut frame);
            // As the frontend's check_closing_animation.
            let light: u64 = frame.iter().map(|p| ((p >> 16) & 0xFF).max((p >> 8) & 0xFF).max(p & 0xFF) as u64).sum();
            let black = light < 4 * frame.len() as u64;
            if black && black_at.is_none() {
                black_at = Some(f as f32 / 60.0);
            }
            if [60, 180, 360, 600, 900].contains(&f) {
                lights.push(format!("{}s:{:.1}", f / 60, light as f64 / frame.len() as f64));
            }
        }
        let name = std::path::Path::new(cart).file_name().unwrap().to_string_lossy();
        println!("{name}: black at {:?} s; mean light {}", black_at, lights.join(" "));
    }
}
