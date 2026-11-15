//! Touches a grid of raw pen ratios and prints where the app felt each one:
//! `lpgrid <bios> <cart> [eeprom|-] [u0,u1,v0,v1,n]` (ratios of the sensor's
//! full signal, -1..1; default -0.95,0.95,-0.95,0.95,5). Each line: u, v,
//! then the app's page, x and y from its `{DT}` log line (or "-"). For
//! working out how a base's pen maps to its book (docs/leappad-family.md).
use ribbit_leappad::{Machine, MACHINE_HZ};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut m = Machine::new(std::fs::read(&a[1]).expect("bios"));
    if a.get(3).is_some_and(|e| e == "mflp") {
        m.bus.eeprom = ribbit_leappad::eeprom::Eeprom::new(ribbit_leappad::calibration::my_first::factory_eeprom());
    } else if let Some(e) = a.get(3).filter(|e| *e != "-") {
        m.bus.eeprom = ribbit_leappad::eeprom::Eeprom::new(std::fs::read(e).expect("eeprom"));
    }
    m.insert_cartridge(std::fs::read(&a[2]).expect("cart"));
    let g: Vec<f32> = a.get(4).map_or("-0.95,0.95,-0.95,0.95,5".into(), |s| s.clone()).split(',').map(|x| x.parse().unwrap()).collect();
    let n = g[4] as usize;
    m.run_cycles(MACHINE_HZ * 12);
    m.take_log();
    for i in 0..n {
        for j in 0..n {
            let t = |k: usize, lo: f32, hi: f32| if n > 1 { lo + (hi - lo) * k as f32 / (n - 1) as f32 } else { lo };
            let (u, v) = (t(i, g[0], g[1]), t(j, g[2], g[3]));
            m.bus.pen.input.position = Some((u, v));
            m.run_cycles(MACHINE_HZ / 4);
            m.bus.pen.input.position = None;
            m.run_cycles(MACHINE_HZ * 3 / 2);
            let log = m.take_log();
            let at = log.find("{DT}").and_then(|p| {
                let s = &log[p..];
                let o = s.find('(')?;
                let c = s[o..].find(')')?;
                Some(s[o + 1..o + c].replace(' ', ""))
            });
            println!("{u:.3}\t{v:.3}\t{}", at.unwrap_or_else(|| "-".into()));
        }
    }
}
