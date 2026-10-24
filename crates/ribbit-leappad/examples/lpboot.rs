//! Boot probe: runs the base ROM (and a cartridge) and reports what the
//! firmware printed on its serial port, the accesses the model does not
//! understand and where the CPU spends its time.
//! `lpboot <bios> [cart|-] [millions of instructions]`
use ribbit_leappad::disasm::disasm;
use ribbit_leappad::Machine;
use ribbit_leappad::mcs51::Bus as _;
use std::collections::HashMap;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut m = Machine::new(std::fs::read(&a[1]).expect("BIOS"));
    if let Some(c) = a.get(2).filter(|c| *c != "-") {
        m.insert_cartridge(std::fs::read(c).expect("cart"));
    }
    let steps: u64 = a.get(3).map(|s| s.parse::<u64>().unwrap() * 1_000_000).unwrap_or(5_000_000);
    // PINHZ=n: the timer pins' clock (lib.rs TIMER_PIN_HZ).
    if let Ok(hz) = std::env::var("PINHZ") {
        m.pin_hz = hz.parse().expect("PINHZ");
    }
    if let Ok(hz) = std::env::var("HZ") {
        m.hz = hz.parse().expect("HZ");
    }
    if std::env::var("FACTORY").is_ok() {
        m.bus.eeprom = ribbit_leappad::eeprom::Eeprom::new(ribbit_leappad::calibration::factory_eeprom());
    }
    if let Ok(e) = std::env::var("EEPROM") {
        m.bus.eeprom = ribbit_leappad::eeprom::Eeprom::new(std::fs::read(e).expect("EEPROM file"));
    }
    m.bus.trace_io = std::env::var("IOTRACE").is_ok();
    // XWATCH=lo,hi;lo,hi (hex): log on-chip RAM writes there.
    if let Ok(x) = std::env::var("XWATCH") {
        for part in x.split(';') {
            let v: Vec<u16> = part.split(',').map(|n| u16::from_str_radix(n, 16).expect("hex")).collect();
            m.bus.xwatch.push((v[0], v[1]));
        }
    }
    // CARTREADS=file: every cartridge offset read, with the serial output
    // printed before it ("<uart length> <offset hex>" per line).
    let cart_reads = std::env::var("CARTREADS").ok();
    if cart_reads.is_some() {
        m.bus.cart_reads = Some(Vec::new());
    }
    if std::env::var("DSPLOG").is_ok() {
        m.bus.dsp_log = Some(Vec::new());
    }
    if std::env::var("EELOG").is_ok() {
        m.bus.eeprom.log = Some(Vec::new());
    }
    let mut vectors: HashMap<u16, u64> = HashMap::new();
    let mut dsp_times: Vec<f64> = Vec::new();
    let mut hist: HashMap<(u32, u16), u64> = HashMap::new();
    let mut recent = std::collections::VecDeque::new();
    // PEN=u,v,start,length: hold the pen at axis ratios (u, v) from `start`
    // for `length` (millions of instructions).
    let pen: Option<(f32, f32, u64, u64)> = std::env::var("PEN").ok().map(|p| {
        let f: Vec<f64> = p.split(',').map(|x| x.parse().expect("PEN=u,v,start,length")).collect();
        (f[0] as f32, f[1] as f32, (f[2] * 1e6) as u64, (f[3] * 1e6) as u64)
    });
    // PAGEPEN=page,x,y,start,length: the same at a point on a page, in the
    // cartridge's page units (see calibration.rs).
    let pen = pen.or_else(|| {
        std::env::var("PAGEPEN").ok().map(|p| {
            let f: Vec<f64> = p.split(',').map(|x| x.parse().expect("PAGEPEN=page,x,y,start,length")).collect();
            let page = if f[0] == 0.0 { ribbit_leappad::book::Side::Left } else { ribbit_leappad::book::Side::Right };
            let (u, v) = ribbit_leappad::book::pen_ratios(ribbit_leappad::book::Spot { page, x: f[1] as f32, y: f[2] as f32 });
            (u, v, (f[3] * 1e6) as u64, (f[4] * 1e6) as u64)
        })
    });
    // WATCH=lo,hi (hex): report instructions that read internal RAM lo..=hi.
    let watch: Option<(u8, u8)> = std::env::var("WATCH").ok().map(|w| {
        let v: Vec<u8> = w.split(',').map(|x| u8::from_str_radix(x, 16).expect("hex")).collect();
        (v[0], v[1])
    });
    let mut watched: Vec<(u32, u16, u8)> = Vec::new();
    let brk: Option<u32> = std::env::var("BREAK").ok().map(|b| u32::from_str_radix(&b, 16).expect("hex"));
    let mut trail: std::collections::VecDeque<(u32, u16, u16)> = std::collections::VecDeque::new();
    // TRAIL=n: how many instructions BREAK/BREAKTEXT show (300).
    let trail_len: usize = std::env::var("TRAIL").ok().map(|t| t.parse().expect("TRAIL=n")).unwrap_or(300);
    // TRAILFROM=text: the trail starts again each time the log prints text.
    let trail_from = std::env::var("TRAILFROM").ok();
    let mut from_seen = 0;
    let brk_text = std::env::var("BREAKTEXT").ok();
    // BREAKDSP=n: break once the sound processor has had n commands.
    let brk_dsp: Option<usize> = std::env::var("BREAKDSP").ok().map(|n| n.parse().expect("BREAKDSP=n"));
    if brk_dsp.is_some() {
        m.bus.dsp_log.get_or_insert_with(Vec::new);
    }
    let mut uart_seen = 0;
    // HITS=phys,phys,... (hex): how often each address runs.
    let hit_at: Vec<u32> = std::env::var("HITS").map(|h| h.split(',').map(|x| u32::from_str_radix(x, 16).expect("hex")).collect()).unwrap_or_default();
    let mut hits = vec![0u64; hit_at.len()];
    // WAV=file: the sound processor's 8 kHz mix.
    let wav = std::env::var("WAV").ok();
    let mut audio: Vec<i16> = Vec::new();
    for step in 0..steps {
        if wav.is_some() && step % 1000 == 0 {
            let mut buf = vec![0i16; m.bus.sound.output.len()];
            m.bus.sound.output.pop_into(&mut buf);
            audio.extend_from_slice(&buf);
        }
        if !hit_at.is_empty() {
            let phys = m.bus.window_phys((m.cpu.pc >> 15) as usize, m.cpu.pc);
            if let Some(i) = hit_at.iter().position(|&a| a == phys) {
                hits[i] += 1;
            }
        }
        if brk.is_some() || brk_text.is_some() || brk_dsp.is_some() {
            let phys = m.bus.window_phys((m.cpu.pc >> 15) as usize, m.cpu.pc);
            let text_hit = brk_text.as_ref().is_some_and(|t| {
                let hit = m.bus.uart.len() != uart_seen && String::from_utf8_lossy(&m.bus.uart[uart_seen.saturating_sub(t.len())..]).contains(t.as_str());
                uart_seen = m.bus.uart.len();
                hit
            });
            let dsp_hit = brk_dsp.is_some_and(|n| m.bus.dsp_log.as_ref().is_some_and(|l| l.len() >= n));
            if Some(phys) == brk || text_hit || dsp_hit {
                println!("--- BREAK at {phys:07X} after {step} instructions; trail (phys pc: instruction) ---");
                for &(p, pc, dptr) in &trail {
                    let (t, _) = disasm(pc, |x| m.bus.code(x));
                    println!("  {p:07X} {pc:04X}: {t:<24} DPTR={dptr:04X}");
                }
                let r: Vec<String> = (0..8).map(|n| format!("R{n}={:02X}", m.cpu.r(n))).collect();
                println!("{} A={:02X} DPTR={:04X}", r.join(" "), m.cpu.acc(), m.cpu.dptr());
                let i = &m.cpu.iram;
                let at = u16::from_be_bytes([i[0x53], i[0x54]]);
                let bytes: Vec<String> = (0..24u16).map(|k| format!("{:02X}", m.bus.xread(at.wrapping_add(k)))).collect();
                println!("IRAM 4A={:02X} 53:54={at:04X} -> {} (window 2 bank {:02X?}, 3 {:02X?})", i[0x4A], bytes.join(" "), m.bus.banks[2], m.bus.banks[3]);
                let x: Vec<String> = (0x460..0x4F8).map(|k| format!("{:02X}", m.bus.xram[k])).collect();
                for (r, chunk) in x.chunks(16).enumerate() {
                    println!("XRAM {:04X}: {}", 0xF460 + 16 * r, chunk.join(" "));
                }
                println!("SP {:02X} stack {:02X?}", m.cpu.sp(), &m.cpu.iram[0xA0..=m.cpu.sp() as usize]);
                println!("IRAM 60-7F {:02X?}", &m.cpu.iram[0x60..0x80]);
                return;
            }
            if let Some(t) = &trail_from
                && m.bus.uart.len() != from_seen
            {
                if String::from_utf8_lossy(&m.bus.uart[from_seen.saturating_sub(t.len())..]).contains(t.as_str()) {
                    trail.clear();
                }
                from_seen = m.bus.uart.len();
            }
            trail.push_back((phys, m.cpu.pc, m.cpu.dptr()));
            if trail.len() > trail_len {
                trail.pop_front();
            }
        }
        if let Some((lo, hi)) = watch {
            let pc = m.cpu.pc;
            let op = m.bus.code(pc);
            let b1 = m.bus.code(pc.wrapping_add(1));
            // Opcodes whose first operand byte is a direct address read.
            let direct = matches!(op, 0x05 | 0x15 | 0x25 | 0x35 | 0x45 | 0x55 | 0x65 | 0x85 | 0x95 | 0xA6 | 0xA7
                | 0xA8..=0xAF | 0xB5 | 0xC0 | 0xC5 | 0xD5 | 0xE5);
            let ind = matches!(op & 0x0F, 6 | 7) && !matches!(op, 0x76 | 0x77 | 0xF6 | 0xF7 | 0xA6 | 0xA7 | 0x86 | 0x87)
                || matches!(op, 0x86 | 0x87);
            let hit = (direct && (lo..=hi).contains(&b1))
                || (ind && (lo..=hi).contains(&m.cpu.iram[((m.cpu.peek_sfr(0xD0) & 0x18) + (op & 1)) as usize]));
            if hit && watched.len() < 40 {
                let phys = m.bus.window_phys((pc >> 15) as usize, pc);
                if !watched.iter().any(|w| w.0 == phys) {
                    watched.push((phys, pc, (step / 1_000_000) as u8));
                }
            }
        }
        if let Some((u, v, at, len)) = pen {
            let down = step >= at && step < at + len;
            m.bus.pen.input.position = down.then_some((u, v));
            if std::env::var("PENDUMP").is_ok() && step + 2_000_000 >= at && step < at + len + 2_000_000 && step % 1_000_000 == 0 {
                let i = &m.cpu.iram;
                let x = &m.bus.xram;
                let w = |a: usize| u16::from_be_bytes([x[a - 0xF000], x[a - 0xF000 + 1]]);
                let samples: Vec<String> = (0..6).map(|k| format!("{:04X}", w(0xF0DF + 2 * k))).collect();
                let line = format!(
                    "
[pen {}M] 1B={:02X} 1C={:02X} 19={:02X} 20={:02X} 22={:02X} thr={:02X}{:02X} off={:02X}{:02X} out={:02X}{:02X},{:02X}{:02X} prev={:02X}{:02X},{:02X}{:02X},{:02X}{:02X} s={} | {} IE={:02X} TCON={:02X} PC={:04X}",
                    step / 1_000_000, i[0x1B], i[0x1C], i[0x19], i[0x20], i[0x22], i[0x2C], i[0x2D], i[0x1D], i[0x1E],
                    i[0x28], i[0x29], i[0x2A], i[0x2B], i[0x13], i[0x14], i[0x15], i[0x16], i[0x17], i[0x18], samples.join(" "), m.bus.pen.describe(), m.cpu.peek_sfr(0xA8), m.cpu.peek_sfr(0x88), m.cpu.pc);
                m.bus.uart.extend_from_slice(line.as_bytes());
            }
            if std::env::var("PENLOG").is_ok() && step == at.saturating_sub(1_000_000) {
                m.bus.pen.log = Some(Vec::new());
            }
            if std::env::var("IOSEQ").is_ok() && step == at {
                m.bus.io_trace.clear();
                m.bus.trace_io = true;
            }
            if step == at || step == at + len {
                m.bus.uart.extend_from_slice(if down { b"
<<pen down>>
" } else { b"
<<pen up>>
" });
            }
        }
        let phys = m.bus.window_phys((m.cpu.pc >> 15) as usize, m.cpu.pc);
        *hist.entry((phys, m.cpu.pc)).or_default() += 1;
        recent.push_back(m.cpu.pc);
        if recent.len() > 40 {
            recent.pop_front();
        }
        if step % 1_000_000 == 0 && std::env::var("TIMES").is_ok() {
            let t = format!("
[{}M instrs = {:.2} s]", step / 1_000_000, m.cpu.cycles as f64 / m.hz as f64);
            m.bus.uart.extend_from_slice(t.as_bytes());
        }
        let before = m.cpu.pc;
        m.step();
        // When each sound command was sent, in seconds of emulated time.
        if let Some(l) = &m.bus.dsp_log
            && l.len() > dsp_times.len()
        {
            dsp_times.resize(l.len(), m.cpu.cycles as f64 / m.hz as f64);
        }
        if m.cpu.pc < 0x33 && m.cpu.pc & 7 == 3 && before != m.cpu.pc.wrapping_sub(2) {
            *vectors.entry(m.cpu.pc).or_default() += 1;
        }
    }
    if let Some(path) = &wav {
        let mut f = Vec::new();
        let data = (audio.len() * 2) as u32;
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&(36 + data).to_le_bytes());
        f.extend_from_slice(b"WAVEfmt ");
        for v in [16u32, 1 | 1 << 16, 8000, 16000, 2 | 16 << 16] {
            f.extend_from_slice(&v.to_le_bytes());
        }
        f.extend_from_slice(b"data");
        f.extend_from_slice(&data.to_le_bytes());
        for s in &audio {
            f.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, f).expect("write WAV");
        let loud = audio.iter().filter(|s| s.unsigned_abs() > 64).count();
        println!("--- sound: {} samples ({:.1} s), {:.1} s not silent -> {path} ---", audio.len(), audio.len() as f64 / 8000.0, loud as f64 / 8000.0);
    }
    // XDUMP=addr,len (hex): on-chip XDATA at the end.
    if let Ok(x) = std::env::var("XDUMP") {
        for part in x.split(';') {
            let v: Vec<usize> = part.split(',').map(|n| usize::from_str_radix(n, 16).expect("hex")).collect();
            let bytes: Vec<String> = (v[0]..v[0] + v[1]).map(|a| format!("{:02X}", m.bus.xram[a - 0xF000])).collect();
            println!("XDATA {:04X}: {}", v[0], bytes.join(" "));
        }
    }
    for (a, n) in hit_at.iter().zip(&hits) {
        println!("hits {a:07X}: {n}");
    }
    if !m.bus.xwatch_log.is_empty() {
        println!("--- watched RAM writes (pc bank addr=value), {} ---", m.bus.xwatch_log.len());
        let mut agg: std::collections::BTreeMap<(u16, u16), (u32, std::collections::BTreeSet<u16>)> = Default::default();
        for &(pc, bank, a, _) in &m.bus.xwatch_log {
            let e = agg.entry((bank, pc)).or_default();
            e.0 += 1;
            if e.1.len() < 8 { e.1.insert(a); }
        }
        for ((bank, pc), (n, addrs)) in &agg {
            println!("  by pc: b{bank:03X} {pc:04X} x{n} {addrs:04X?}");
        }
        for (pc, bank, a, v) in m.bus.xwatch_log.iter().take(if std::env::var("XWATCHALL").is_ok() { 400 } else { 0 }) {
            println!("  {pc:04X} b{bank:03X} {a:04X}={v:02X}");
        }
    }
    if !watched.is_empty() {
        println!("--- reads of the watched RAM (phys, pc, at M instrs) ---");
        for (p, pc, at) in &watched {
            println!("  {p:07X} {pc:04X} @{at}M");
        }
    }
    if let Some(l) = &m.bus.dsp_log {
        println!("--- sound processor commands ({}): cmd start end/loop pitch F4B8 vol F4D2 ---", l.len());
        for (i, (pc, c, a)) in l.iter().enumerate() {
            let h = |r: &[u8]| r.iter().map(|b| format!("{b:02X}")).collect::<String>();
            print!("{:9.3}s", dsp_times.get(i).copied().unwrap_or(0.0));
            println!("  {pc:04X} {c:02X} {} {} {} {} {} {}", h(&a[0..4]), h(&a[4..8]), h(&a[8..10]), h(&a[10..12]), h(&a[12..14]), h(&a[14..16]));
        }
    }
    if let Some(l) = &m.bus.eeprom.log {
        println!("--- EEPROM bus ({} events) ---
{}", l.len(), l.iter().cloned().collect::<Vec<_>>().join(" "));
    }
    if std::env::var("IOSEQ").is_ok() {
        println!("--- register sequence from the press ---");
        for &(pc, reg, w, v) in m.bus.io_trace.iter().filter(|e| matches!(e.1, 0xFC16..=0xFC19 | 0xFC2A)).take(60) {
            println!("  {pc:04X} {reg:04X} {} {v:02X}", if w { "W" } else { "R" });
        }
    }
    if let Some(l) = &m.bus.pen.log {
        let hex: Vec<String> = l.iter().take(120).map(|c| format!("{c:02X}")).collect();
        println!("--- pen codes queued ({} in all) ---
{}", l.len(), hex.join(" "));
    }
    if let (Some(f), Some(r)) = (&cart_reads, &m.bus.cart_reads) {
        let text: String = r.iter().map(|(u, a)| format!("{u} {a:X}
")).collect();
        std::fs::write(f, text).expect("CARTREADS file");
    }
    let (cpu, mut bus) = (m.cpu, m.bus);
    println!("--- serial output ---\n{}", String::from_utf8_lossy(&bus.uart));
    println!("--- unknown accesses (first {}) ---", bus.unknown.len().min(200));
    for u in bus.unknown.iter().take(200) {
        println!("  {:04X}: {}", u.pc, u.what);
    }
    let mut h: Vec<_> = hist.into_iter().collect();
    h.sort_by(|a, b| b.1.cmp(&a.1));
    println!("--- hottest PCs ---");
    for ((p, pc), n) in h.iter().take(12) {
        println!("  {pc:04X} (phys {p:06X}) x{n}");
    }
    println!("--- last instructions ---");
    for pc in recent {
        let (t, _) = disasm(pc, |x| bus.code(x));
        println!("  {pc:04X}: {t}");
    }
    println!("--- interrupt vectors taken --- {vectors:X?}");
    if bus.trace_io {
        let mut agg: HashMap<(u16, u16, bool), (u64, std::collections::BTreeSet<u8>)> = HashMap::new();
        for &(pc, reg, w, v) in &bus.io_trace {
            let e = agg.entry((pc, reg, w)).or_default();
            e.0 += 1;
            if e.1.len() < 12 {
                e.1.insert(v);
            }
        }
        let mut v: Vec<_> = agg.into_iter().collect();
        v.sort_by_key(|((pc, reg, _), _)| (*reg, *pc));
        println!("--- register accesses: pc reg r/w count values ---");
        for ((pc, reg, w), (n, vals)) in v {
            println!("  {pc:04X} {reg:04X} {} x{n:<8} {vals:02X?}", if w { "W" } else { "R" });
        }
    }
    println!("banks {:02X?}  cycles {}  SP {:02X}", bus.banks, cpu.cycles, cpu.sp());
}
