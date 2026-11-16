//! Rust answers to the library's imports (bionic libc/libm, pthread, liblog, libdl...), by
//! name. Each reads its arguments with `Runtime::arg` and returns with `Runtime::ret`. An
//! import that isn't here stops the run (`Stop::Import`), so they are added as titles reach
//! them. One thread so far: locks always succeed, `pthread_once` runs its routine inline.

use crate::runtime::{Runtime, Stop};

/// What the imports keep between calls.
#[derive(Default)]
pub struct State {
    /// pthread_once controls already run.
    pub once: std::collections::BTreeSet<u32>,
    /// `__sF`: stdin, stdout, stderr (FILE is 84 bytes in bionic).
    pub sf: u32,
    /// `__errno()`'s location.
    pub errno_at: u32,
    /// lrand48's state.
    pub rand: u64,
    /// zlib's inflate streams, by `z_stream` address.
    pub inflate: std::collections::BTreeMap<u32, Box<miniz_oxide::inflate::stream::InflateState>>,
}

/// Fills the slot of a data import with what the code reads there.
pub fn init_data(rt: &mut Runtime, name: &str, slot: u32) {
    match name {
        "__stack_chk_guard" => rt.mem.w32(slot, 0x5EED_CAFE),
        // stdin, stdout and stderr are slot, slot + 84 and slot + 168, only ever handed back
        // to stdio here.
        "__sF" => rt.state.sf = slot,
        // bionic's `const char *_ctype_` (OpenBSD's) points to the table: [1 + c] holds c's
        // class bits (EOF at 0). The slot holds the pointer, the table follows it.
        // libstdc++'s ctype<char>::classic_table() is `_ctype_ + 1`.
        "_ctype_" => {
            rt.mem.w32(slot, slot + 4);
            for c in 0..=255u8 {
                let ch = c as char;
                let bits = [
                    (ch.is_ascii_uppercase(), 0x01),
                    (ch.is_ascii_lowercase(), 0x02),
                    (ch.is_ascii_digit(), 0x04),
                    (matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C), 0x08),
                    (ch.is_ascii_punctuation(), 0x10),
                    (ch.is_ascii_control(), 0x20),
                    (ch.is_ascii_hexdigit(), 0x40),
                    (c == b' ', 0x80),
                ];
                let f = bits.iter().filter(|b| b.0).fold(0u8, |f, b| f | b.1);
                rt.mem.w8(slot + 5 + c as u32, f);
            }
        }
        // `const short *_tolower_tab_` / `_toupper_tab_`: [1 + c] is c mapped (EOF -1 at 0).
        "_tolower_tab_" | "_toupper_tab_" => {
            rt.mem.w32(slot, slot + 4);
            rt.mem.w16(slot + 4, 0xFFFF);
            for c in 0..=255u8 {
                let m = if name == "_tolower_tab_" { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() };
                rt.mem.w16(slot + 6 + 2 * c as u32, m as u16);
            }
        }
        // OpenSL ES / OpenMAX AL interface IDs: pointers to 16-byte IDs, each unique.
        n if n.starts_with("SL_IID_") || n.starts_with("XA_IID_") => {
            let id = rt.malloc(16);
            rt.mem.write(id, &n.as_bytes()[..n.len().min(16)]);
            rt.mem.w32(slot, id);
        }
        _ => {}
    }
}

fn f64_arg(rt: &mut Runtime, i: usize) -> f64 {
    f64::from_bits(rt.arg(i) as u64 | (rt.arg(i + 1) as u64) << 32)
}
fn f32_arg(rt: &mut Runtime, i: usize) -> f32 {
    f32::from_bits(rt.arg(i))
}

/// Where printf-style arguments come from: the call's own (from argument `i`), or a va_list.
enum Args {
    Call(usize),
    Va(u32),
}

impl Args {
    fn u32(&mut self, rt: &mut Runtime) -> u32 {
        match self {
            Args::Call(i) => {
                let v = rt.arg(*i);
                *i += 1;
                v
            }
            Args::Va(p) => {
                let v = rt.mem.r32(*p);
                *p += 4;
                v
            }
        }
    }
    fn u64(&mut self, rt: &mut Runtime) -> u64 {
        match self {
            Args::Call(i) => *i = i.next_multiple_of(2),
            Args::Va(p) => *p = p.next_multiple_of(8),
        }
        let lo = self.u32(rt) as u64;
        lo | (self.u32(rt) as u64) << 32
    }
}

/// C's printf formatting: flags, width, precision, the h/l/ll/z/j/t lengths, and
/// d i u o x X c s p f F e E g G %.
fn format(rt: &mut Runtime, fmt: &str, mut args: Args) -> String {
    let mut out = String::new();
    let mut it = fmt.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut flags = String::new();
        while let Some(&f) = it.peek() {
            if "-+ #0".contains(f) {
                flags.push(f);
                it.next();
            } else {
                break;
            }
        }
        let num = |it: &mut std::iter::Peekable<std::str::Chars>, args: &mut Args, rt: &mut Runtime| -> Option<usize> {
            if it.peek() == Some(&'*') {
                it.next();
                return Some(args.u32(rt) as i32 as usize);
            }
            let mut n = None;
            while let Some(d) = it.peek().and_then(|c| c.to_digit(10)) {
                n = Some(n.unwrap_or(0) * 10 + d as usize);
                it.next();
            }
            n
        };
        let width = num(&mut it, &mut args, rt);
        let prec = if it.peek() == Some(&'.') {
            it.next();
            Some(num(&mut it, &mut args, rt).unwrap_or(0))
        } else {
            None
        };
        let mut long = 0;
        while let Some(&l) = it.peek() {
            match l {
                'l' | 'q' | 'L' => long += 1,
                'j' => long = 2,
                'h' | 'z' | 't' => {}
                _ => break,
            }
            it.next();
        }
        let Some(conv) = it.next() else { break };
        let s = match conv {
            'd' | 'i' => {
                let v = if long >= 2 { args.u64(rt) as i64 } else { args.u32(rt) as i32 as i64 };
                let mut s = v.abs().to_string();
                if let Some(p) = prec {
                    while s.len() < p {
                        s.insert(0, '0');
                    }
                }
                if v < 0 {
                    s.insert(0, '-');
                } else if flags.contains('+') {
                    s.insert(0, '+');
                } else if flags.contains(' ') {
                    s.insert(0, ' ');
                }
                s
            }
            'u' | 'x' | 'X' | 'o' => {
                let v = if long >= 2 { args.u64(rt) } else { args.u32(rt) as u64 };
                let mut s = match conv {
                    'u' => v.to_string(),
                    'x' => format!("{v:x}"),
                    'X' => format!("{v:X}"),
                    _ => format!("{v:o}"),
                };
                if let Some(p) = prec {
                    while s.len() < p {
                        s.insert(0, '0');
                    }
                }
                if flags.contains('#') && v != 0 {
                    s.insert_str(0, match conv {
                        'x' => "0x",
                        'X' => "0X",
                        'o' => "0",
                        _ => "",
                    });
                }
                s
            }
            'p' => format!("{:#x}", args.u32(rt)),
            'c' => char::from(args.u32(rt) as u8).to_string(),
            's' => {
                let a = args.u32(rt);
                let mut s = if a == 0 { "(null)".to_string() } else { rt.mem.cstr(a) };
                if let Some(p) = prec {
                    s = s.chars().take(p).collect();
                }
                s
            }
            'f' | 'F' | 'e' | 'E' | 'g' | 'G' => {
                let v = f64::from_bits(args.u64(rt));
                let p = prec.unwrap_or(6);
                match conv {
                    'f' | 'F' => format!("{v:.p$}"),
                    'e' => format!("{v:.p$e}"),
                    'E' => format!("{v:.p$E}"),
                    _ => format!("{v}"),
                }
            }
            '%' => "%".to_string(),
            other => format!("%{other}"),
        };
        let w = width.unwrap_or(0);
        if s.chars().count() < w {
            let pad = w - s.chars().count();
            if flags.contains('-') {
                out.push_str(&s);
                out.extend(std::iter::repeat_n(' ', pad));
            } else if flags.contains('0') && !"sc".contains(conv) {
                let (sign, digits) = if s.starts_with(['-', '+', ' ']) { s.split_at(1) } else { ("", s.as_str()) };
                out.push_str(sign);
                out.extend(std::iter::repeat_n('0', pad));
                out.push_str(digits);
            } else {
                out.extend(std::iter::repeat_n(' ', pad));
                out.push_str(&s);
            }
        } else {
            out.push_str(&s);
        }
    }
    out
}

/// Writes `s` (cut to `n` - 1 bytes and NUL-terminated, as snprintf) at `buf`; returns the
/// full length.
fn put_str(rt: &mut Runtime, buf: u32, n: Option<u32>, s: &str) -> u32 {
    let bytes = s.as_bytes();
    let keep = match n {
        Some(0) => return bytes.len() as u32,
        Some(n) => bytes.len().min(n as usize - 1),
        None => bytes.len(),
    };
    rt.mem.write(buf, &bytes[..keep]);
    rt.mem.w8(buf + keep as u32, 0);
    bytes.len() as u32
}

pub(crate) fn log_line(rt: &mut Runtime, s: String) {
    let s = s.trim_end_matches('\n').to_string();
    if std::env::var_os("LFA_LOG").is_some() {
        eprintln!("[game] {s}");
    }
    // LFA_LOG_BT=<text>: a backtrace for each logged line that has it.
    if std::env::var("LFA_LOG_BT").is_ok_and(|t| s.contains(&t)) {
        eprintln!("  backtrace {:08x?}", rt.backtrace());
    }
    rt.log.push(s);
}

const ANDROID_PRIO: [&str; 8] = ["?", "V", "V", "D", "I", "W", "E", "F"];

/// Answers import `name` (the thunk was reached).
pub fn call(rt: &mut Runtime, name: &str) -> Result<(), Stop> {
    let lr = rt.cpu.r[14];
    match name {
        // ---- memory and strings
        "memcpy" | "memmove" | "__aeabi_memcpy" | "__aeabi_memmove" | "__aeabi_memcpy4" | "__aeabi_memcpy8" | "__memcpy_chk" | "__memmove_chk" => {
            let (d, s, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let data = rt.mem.read(s, n as usize);
            rt.mem.write(d, &data);
            rt.ret(d)
        }
        "memset" | "__memset_chk" => {
            let (d, v, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            for i in 0..n {
                rt.mem.w8(d + i, v as u8);
            }
            rt.ret(d)
        }
        // __aeabi_memset(dest, n, c); __aeabi_memclr(dest, n)
        "__aeabi_memset" | "__aeabi_memset4" | "__aeabi_memset8" | "__aeabi_memclr" | "__aeabi_memclr4" | "__aeabi_memclr8" => {
            let (d, n) = (rt.arg(0), rt.arg(1));
            let v = if name.contains("clr") { 0 } else { rt.arg(2) as u8 };
            for i in 0..n {
                rt.mem.w8(d + i, v);
            }
            rt.ret(d)
        }
        "memcmp" => {
            let (a, b, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let mut r = 0i32;
            for i in 0..n {
                let (x, y) = (rt.mem.r8(a + i), rt.mem.r8(b + i));
                if x != y {
                    r = x as i32 - y as i32;
                    break;
                }
            }
            rt.ret(r as u32)
        }
        "memchr" => {
            let (a, c, n) = (rt.arg(0), rt.arg(1) as u8, rt.arg(2));
            let r = (0..n).find(|&i| rt.mem.r8(a + i) == c).map_or(0, |i| a + i);
            rt.ret(r)
        }
        "strlen" | "__strlen_chk" => {
            let a = rt.arg(0);
            let n = (0..).find(|&i| rt.mem.r8(a + i) == 0).unwrap();
            rt.ret(n)
        }
        "strcmp" | "strncmp" | "strcasecmp" | "strncasecmp" => {
            let (a, b) = (rt.arg(0), rt.arg(1));
            let n = if name.contains('n') && name != "strcasecmp" { rt.arg(2) } else { u32::MAX };
            let fold = name.contains("case");
            let mut r = 0i32;
            for i in 0..n {
                let (mut x, mut y) = (rt.mem.r8(a + i), rt.mem.r8(b + i));
                if fold {
                    x = x.to_ascii_lowercase();
                    y = y.to_ascii_lowercase();
                }
                if x != y || x == 0 {
                    r = x as i32 - y as i32;
                    break;
                }
            }
            rt.ret(r as u32)
        }
        "strcpy" | "__strcpy_chk" | "stpcpy" => {
            let (d, s) = (rt.arg(0), rt.arg(1));
            let st = rt.mem.cstr(s);
            rt.mem.write(d, st.as_bytes());
            rt.mem.w8(d + st.len() as u32, 0);
            rt.ret(if name == "stpcpy" { d + st.len() as u32 } else { d })
        }
        "strncpy" | "__strncpy_chk" => {
            let (d, s, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let mut end = false;
            for i in 0..n {
                let b = if end { 0 } else { rt.mem.r8(s + i) };
                end |= b == 0;
                rt.mem.w8(d + i, b);
            }
            rt.ret(d)
        }
        "strcat" | "__strcat_chk" | "strncat" | "__strncat_chk" => {
            let (d, s) = (rt.arg(0), rt.arg(1));
            let len = rt.mem.cstr(s).len();
            let mut add = rt.mem.read(s, len);
            if name.contains('n') {
                add.truncate(rt.arg(2) as usize);
            }
            let at = d + rt.mem.cstr(d).len() as u32;
            rt.mem.write(at, &add);
            rt.mem.w8(at + add.len() as u32, 0);
            rt.ret(d)
        }
        "strchr" | "__strchr_chk" | "strrchr" | "__strrchr_chk" => {
            let (a, c) = (rt.arg(0), rt.arg(1) as u8);
            let len = rt.mem.cstr(a).len() as u32;
            let hits = (0..=len).filter(|&i| rt.mem.r8(a + i) == c);
            let r = if name.contains("rchr") { hits.last() } else { hits.into_iter().next() };
            rt.ret(r.map_or(0, |i| a + i))
        }
        "strstr" => {
            let (a, b) = (rt.arg(0), rt.arg(1));
            let (h, n) = (rt.mem.cstr(a), rt.mem.cstr(b));
            rt.ret(h.find(&n).map_or(0, |i| a + i as u32))
        }
        "strdup" => {
            let a = rt.arg(0);
            let s = rt.mem.cstr(a);
            let r = rt.alloc_str(&s);
            rt.ret(r)
        }
        "tolower" | "toupper" => {
            let c = rt.arg(0) as u8;
            rt.ret(if name == "tolower" { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() } as u32)
        }
        "atoi" | "atol" | "strtol" | "strtoul" => {
            let a = rt.arg(0);
            let s = rt.mem.cstr(a);
            let base = if name.starts_with("strto") { rt.arg(2) } else { 10 };
            let t = s.trim_start();
            let skipped = (s.len() - t.len()) as u32;
            let (neg, t2) = match t.strip_prefix('-') {
                Some(r) => (true, r),
                None => (false, t.strip_prefix('+').unwrap_or(t)),
            };
            let (base, t3) = match base {
                0 | 16 if t2.starts_with("0x") || t2.starts_with("0X") => (16, &t2[2..]),
                0 if t2.starts_with('0') && t2.len() > 1 => (8, &t2[1..]),
                0 => (10, t2),
                b => (b, t2),
            };
            let digits: String = t3.chars().take_while(|c| c.is_digit(base)).collect();
            let v = i64::from_str_radix(&digits, base).unwrap_or(0);
            if name.starts_with("strto") && rt.arg(1) != 0 {
                let end = a + skipped + (t.len() - t3.len() + digits.len()) as u32;
                let ep = rt.arg(1);
                rt.mem.w32(ep, if digits.is_empty() { a } else { end });
            }
            rt.ret(if neg { -v } else { v } as u32)
        }
        "atof" | "strtod" | "strtof" => {
            let a = rt.arg(0);
            let s = rt.mem.cstr(a);
            let t = s.trim_start();
            let len = (1..=t.len()).rev().find(|&l| t[..l].parse::<f64>().is_ok()).unwrap_or(0);
            let v: f64 = t[..len].parse().unwrap_or(0.0);
            if name != "atof" && rt.arg(1) != 0 {
                let ep = rt.arg(1);
                rt.mem.w32(ep, a + (s.len() - t.len() + len) as u32);
            }
            if name == "strtof" { rt.ret((v as f32).to_bits()) } else { rt.ret64(v.to_bits()) }
        }

        // ---- heap
        "malloc" => {
            let n = rt.arg(0);
            let a = rt.malloc(n);
            rt.ret(a)
        }
        "calloc" => {
            let n = rt.arg(0).saturating_mul(rt.arg(1));
            let a = rt.malloc(n);
            for i in 0..n {
                rt.mem.w8(a + i, 0);
            }
            rt.ret(a)
        }
        "realloc" => {
            let (old, n) = (rt.arg(0), rt.arg(1));
            let a = rt.malloc(n);
            if old != 0 {
                let len = rt.allocs.get(&old).copied().unwrap_or(0).min(n);
                let data = rt.mem.read(old, len as usize);
                rt.mem.write(a, &data);
                rt.free(old);
            }
            rt.ret(a)
        }
        "free" => {
            let a = rt.arg(0);
            rt.free(a);
            rt.ret(0)
        }
        "memalign" => {
            let (al, n) = (rt.arg(0).max(16), rt.arg(1));
            let a = rt.malloc(n + al);
            rt.ret(a.next_multiple_of(al))
        }
        "posix_memalign" => {
            let (out, al, n) = (rt.arg(0), rt.arg(1).max(16), rt.arg(2));
            let a = rt.malloc(n + al).next_multiple_of(al);
            rt.mem.w32(out, a);
            rt.ret(0)
        }

        // ---- process
        "__cxa_atexit" | "atexit" | "__cxa_finalize" => rt.ret(0),
        "__errno" => {
            if rt.state.errno_at == 0 {
                rt.state.errno_at = rt.malloc(4);
            }
            let a = rt.state.errno_at;
            rt.ret(a)
        }
        "__stack_chk_fail" => return Err(Stop::Halt(format!("stack smashing detected (from {lr:#x})"))),
        "abort" => return Err(Stop::Halt(format!("abort() from {lr:#x}"))),
        "exit" | "_exit" => return Err(Stop::Halt(format!("exit({})", rt.arg(0) as i32))),
        "__assert2" => {
            let (f, l, func, e) = (rt.arg(0), rt.arg(1), rt.arg(2), rt.arg(3));
            let (f, func, e) = (rt.mem.cstr(f), rt.mem.cstr(func), rt.mem.cstr(e));
            return Err(Stop::Halt(format!("assertion failed: {e} ({func}, {f}:{l})")));
        }
        "getenv" => rt.ret(0),
        "getpid" | "gettid" => rt.ret(1000),
        "sysconf" => {
            let r = match rt.arg(0) {
                // _SC_PAGESIZE
                0x27 | 0x28 => 4096,
                // _SC_NPROCESSORS_CONF / ONLN
                0x60 | 0x61 => 1,
                _ => 0,
            };
            rt.ret(r)
        }

        // ---- time
        "time" => {
            let t = rt.wall_clock().as_secs() as u32;
            let out = rt.arg(0);
            if out != 0 {
                rt.mem.w32(out, t);
            }
            rt.ret(t)
        }
        "gettimeofday" => {
            let d = rt.wall_clock();
            let tv = rt.arg(0);
            rt.mem.w32(tv, d.as_secs() as u32);
            rt.mem.w32(tv + 4, d.subsec_micros());
            rt.ret(0)
        }
        "clock_gettime" => {
            // CLOCK_REALTIME (0) since 1970, the others since the first reading.
            let ts = rt.arg(1);
            let d = if rt.arg(0) == 0 { rt.wall_clock() } else { rt.state_elapsed() };
            rt.mem.w32(ts, d.as_secs() as u32);
            rt.mem.w32(ts + 4, d.subsec_nanos());
            rt.ret(0)
        }

        // ---- pthread (threads.rs has the threads, mutexes and condition variables)
        n if crate::threads::handles(n) => return crate::threads::call(rt, n),
        n if crate::stdio::handles(n) => return crate::stdio::call(rt, n),
        n if crate::files::handles(n) => return crate::files::call(rt, n),
        "pthread_mutex_init" | "pthread_mutexattr_init"
        | "pthread_mutexattr_settype" | "pthread_mutexattr_destroy" | "pthread_cond_init" | "pthread_cond_destroy"
        | "pthread_condattr_init" | "pthread_condattr_destroy" | "pthread_attr_init" | "pthread_attr_destroy" | "pthread_attr_setdetachstate" | "pthread_attr_setstacksize"
        | "pthread_attr_setschedparam" | "pthread_attr_getschedparam" | "pthread_attr_setschedpolicy" | "pthread_setschedparam" | "pthread_rwlock_init"
        | "pthread_rwlock_rdlock" | "pthread_rwlock_wrlock" | "pthread_rwlock_unlock" | "pthread_rwlock_destroy" | "pthread_detach" => rt.ret(0),
        "pthread_equal" => {
            let eq = rt.arg(0) == rt.arg(1);
            rt.ret(eq as u32)
        }
        "pthread_once" => {
            let (ctl, f) = (rt.arg(0), rt.arg(1));
            if rt.state.once.insert(ctl) {
                rt.mem.w32(ctl, 1);
                if let Err(e) = rt.call(f, &[]) {
                    rt.pending = Some(e);
                }
            }
            rt.ret(0)
        }
        "pthread_key_create" => {
            let out = rt.arg(0);
            let k = rt.next_key;
            rt.next_key += 1;
            rt.mem.w32(out, k);
            rt.ret(0)
        }
        "pthread_key_delete" => rt.ret(0),

        // ---- logging and stdio
        "__android_log_print" | "__android_log_vprint" => {
            let (prio, tag, f) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let (tag, f) = (rt.mem.cstr(tag), rt.mem.cstr(f));
            let args = if name.contains("vprint") { Args::Va(rt.arg(3)) } else { Args::Call(3) };
            let s = format(rt, &f, args);
            log_line(rt, format!("{}/{tag}: {s}", ANDROID_PRIO[prio as usize & 7]));
            rt.ret(1)
        }
        "__android_log_write" => {
            let (prio, tag, m) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let (tag, m) = (rt.mem.cstr(tag), rt.mem.cstr(m));
            log_line(rt, format!("{}/{tag}: {m}", ANDROID_PRIO[prio as usize & 7]));
            rt.ret(1)
        }
        "sprintf" | "snprintf" | "vsprintf" | "vsnprintf" | "__sprintf_chk" | "__snprintf_chk" | "__vsprintf_chk" | "__vsnprintf_chk" => {
            let buf = rt.arg(0);
            let sized = name.contains("snprintf");
            let chk = name.ends_with("_chk");
            // (buf, [n], [flag, size], fmt, ...)
            let mut i = 1;
            let n = if sized {
                i += 1;
                Some(rt.arg(1))
            } else {
                None
            };
            if chk {
                i += if sized { 2 } else { 2 };
            }
            let f = rt.arg(i);
            let f = rt.mem.cstr(f);
            let args = if name.contains("vs") { Args::Va(rt.arg(i + 1)) } else { Args::Call(i + 1) };
            let s = format(rt, &f, args);
            let r = put_str(rt, buf, n, &s);
            rt.ret(r)
        }
        // asprintf(char** out, fmt, ...), vasprintf(out, fmt, va): a new string
        "asprintf" | "vasprintf" => {
            let (out, f) = (rt.arg(0), rt.arg(1));
            let f = rt.mem.cstr(f);
            let args = if name == "vasprintf" { Args::Va(rt.arg(2)) } else { Args::Call(2) };
            let s = format(rt, &f, args);
            let a = rt.alloc_str(&s);
            rt.mem.w32(out, a);
            rt.ret(s.len() as u32)
        }
        "printf" | "vprintf" => {
            let f = rt.arg(0);
            let f = rt.mem.cstr(f);
            let args = if name == "vprintf" { Args::Va(rt.arg(1)) } else { Args::Call(1) };
            let s = format(rt, &f, args);
            let n = s.len() as u32;
            log_line(rt, s);
            rt.ret(n)
        }
        "fprintf" | "vfprintf" => {
            let f = rt.arg(1);
            let f = rt.mem.cstr(f);
            let args = if name == "vfprintf" { Args::Va(rt.arg(2)) } else { Args::Call(2) };
            let s = format(rt, &f, args);
            let n = s.len() as u32;
            let fp = rt.arg(0);
            if !rt.stream_put(fp, s.as_bytes()) {
                log_line(rt, s);
            }
            rt.ret(n)
        }
        "puts" => {
            let a = rt.arg(0);
            let s = rt.mem.cstr(a);
            log_line(rt, s);
            rt.ret(1)
        }
        "fputs" => {
            let (a, fp) = (rt.arg(0), rt.arg(1));
            let s = rt.mem.cstr(a);
            if !rt.stream_put(fp, s.as_bytes()) {
                log_line(rt, s);
            }
            rt.ret(1)
        }
        "putchar" => rt.ret(0),

        // ---- OpenGL ES 1.1 and EGL (gles.rs)
        n if n.starts_with("gl") || n.starts_with("egl") => return crate::gles::call(rt, n),

        // ---- OpenSL ES, OpenMAX AL (sles.rs)
        "slCreateEngine" => crate::sles::create_engine(rt, "sl"),
        "xaCreateEngine" => crate::sles::create_engine(rt, "xa"),

        // ---- libdl: nothing else is loaded
        "dlopen" | "dlsym" => rt.ret(0),
        "dlclose" => rt.ret(0),
        "dlerror" => {
            let s = rt.alloc_str("not available");
            rt.ret(s)
        }

        // ---- more strings, wide characters, locale
        "bzero" => {
            let (d, n) = (rt.arg(0), rt.arg(1));
            for i in 0..n {
                rt.mem.w8(d + i, 0);
            }
            rt.ret(0)
        }
        "bcopy" => {
            let (src, d, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let data = rt.mem.read(src, n as usize);
            rt.mem.write(d, &data);
            rt.ret(0)
        }
        "strcoll" => {
            let (a, b) = (rt.arg(0), rt.arg(1));
            let (x, y) = (rt.mem.cstr(a), rt.mem.cstr(b));
            rt.ret(x.cmp(&y) as i32 as u32)
        }
        "strxfrm" => {
            let (d, src, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let st = rt.mem.cstr(src);
            let r = put_str(rt, d, Some(n), &st);
            rt.ret(r)
        }
        "strcasestr" => {
            let (a, b) = (rt.arg(0), rt.arg(1));
            let (h, n) = (rt.mem.cstr(a).to_ascii_lowercase(), rt.mem.cstr(b).to_ascii_lowercase());
            rt.ret(h.find(&n).map_or(0, |i| a + i as u32))
        }
        "strtok_r" => {
            let (s0, delim, save) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let delim = rt.mem.cstr(delim).into_bytes();
            let mut p = if s0 != 0 { s0 } else { rt.mem.r32(save) };
            while p != 0 && rt.mem.r8(p) != 0 && delim.contains(&rt.mem.r8(p)) {
                p += 1;
            }
            if p == 0 || rt.mem.r8(p) == 0 {
                rt.mem.w32(save, 0);
                rt.ret(0);
                return Ok(());
            }
            let start = p;
            while rt.mem.r8(p) != 0 && !delim.contains(&rt.mem.r8(p)) {
                p += 1;
            }
            let next = if rt.mem.r8(p) != 0 {
                rt.mem.w8(p, 0);
                p + 1
            } else {
                0
            };
            rt.mem.w32(save, next);
            rt.ret(start)
        }
        "strerror" => {
            let e = rt.arg(0);
            let s = rt.alloc_str(&format!("error {e}"));
            rt.ret(s)
        }
        "isprint" => {
            let c = rt.arg(0);
            rt.ret((0x20..0x7F).contains(&c) as u32)
        }
        "towlower" | "towupper" => {
            let c = char::from_u32(rt.arg(0)).unwrap_or('\0');
            let r = if name == "towlower" { c.to_lowercase().next() } else { c.to_uppercase().next() }.unwrap_or(c);
            rt.ret(r as u32)
        }
        // wctype(name): a class number for iswctype(c, class).
        "wctype" => {
            let a = rt.arg(0);
            let n = rt.mem.cstr(a);
            let k = ["alnum", "alpha", "blank", "cntrl", "digit", "graph", "lower", "print", "punct", "space", "upper", "xdigit"].iter().position(|&x| x == n).map_or(0, |i| i as u32 + 1);
            rt.ret(k)
        }
        "iswctype" => {
            let (c, k) = (char::from_u32(rt.arg(0)).unwrap_or('\0'), rt.arg(1));
            let r = match k {
                1 => c.is_alphanumeric(),
                2 => c.is_alphabetic(),
                3 => c == ' ' || c == '\t',
                4 => c.is_control(),
                5 => c.is_ascii_digit(),
                6 => !c.is_whitespace() && !c.is_control(),
                7 => c.is_lowercase(),
                8 => !c.is_control(),
                9 => c.is_ascii_punctuation(),
                10 => c.is_whitespace(),
                11 => c.is_uppercase(),
                12 => c.is_ascii_hexdigit(),
                _ => false,
            };
            rt.ret(r as u32)
        }
        // The "C" locale: a byte below 0x80 is its own wide character.
        "wctob" | "btowc" => {
            let c = rt.arg(0);
            rt.ret(if c < 0x80 { c } else { u32::MAX })
        }
        "mbrtowc" => {
            let (pwc, s0, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            if s0 == 0 {
                rt.ret(0);
                return Ok(());
            }
            if n == 0 {
                rt.ret(-2i32 as u32);
                return Ok(());
            }
            let c = rt.mem.r8(s0) as u32;
            if pwc != 0 {
                rt.mem.w32(pwc, c);
            }
            rt.ret(if c == 0 { 0 } else { 1 })
        }
        "wcrtomb" => {
            let (s0, wc) = (rt.arg(0), rt.arg(1));
            if s0 != 0 {
                rt.mem.w8(s0, wc as u8);
            }
            rt.ret(1)
        }
        "wcslen" => {
            let a = rt.arg(0);
            let n = (0..).find(|&i| rt.mem.r32(a + 4 * i) == 0).unwrap();
            rt.ret(n)
        }
        "wcsncpy" => {
            // Copies up to n characters and pads with zeros to n.
            let (d, src, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let mut end = false;
            for i in 0..n {
                let c = if end { 0 } else { rt.mem.r32(src + 4 * i) };
                end |= c == 0;
                rt.mem.w32(d + 4 * i, c);
            }
            rt.ret(d)
        }
        "wcscat" => {
            let (d, src) = (rt.arg(0), rt.arg(1));
            let at = d + 4 * (0..).find(|&i| rt.mem.r32(d + 4 * i) == 0).unwrap();
            for i in 0.. {
                let c = rt.mem.r32(src + 4 * i);
                rt.mem.w32(at + 4 * i, c);
                if c == 0 {
                    break;
                }
            }
            rt.ret(d)
        }
        "wcschr" | "wcsrchr" => {
            let (a, c) = (rt.arg(0), rt.arg(1));
            let mut found = 0;
            for i in 0.. {
                let x = rt.mem.r32(a + 4 * i);
                if x == c {
                    found = a + 4 * i;
                    if name == "wcschr" {
                        break;
                    }
                }
                if x == 0 {
                    break;
                }
            }
            rt.ret(found)
        }
        "wcsstr" => {
            let (h, n) = (rt.arg(0), rt.arg(1));
            let wide = |rt: &mut Runtime, a: u32| -> Vec<u32> { (0..).map(|i| rt.mem.r32(a + 4 * i)).take_while(|&c| c != 0).collect() };
            let (hay, needle) = (wide(rt, h), wide(rt, n));
            let r = if needle.is_empty() { Some(0) } else { hay.windows(needle.len()).position(|w| w == needle.as_slice()) };
            rt.ret(r.map_or(0, |i| h + 4 * i as u32))
        }
        // Formatting dates as wide text: nothing written (the locale's names aren't kept).
        "wcsftime" => rt.ret(0),
        "wmemcpy" | "wmemmove" => {
            let (d, src, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let data = rt.mem.read(src, 4 * n as usize);
            rt.mem.write(d, &data);
            rt.ret(d)
        }
        "wmemset" => {
            let (d, c, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            for i in 0..n {
                rt.mem.w32(d + 4 * i, c);
            }
            rt.ret(d)
        }
        "wmemchr" => {
            let (a, c, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let r = (0..n).find(|&i| rt.mem.r32(a + 4 * i) == c).map_or(0, |i| a + 4 * i);
            rt.ret(r)
        }
        "wmemcmp" | "wcscoll" => {
            let (a, b) = (rt.arg(0), rt.arg(1));
            let n = if name == "wmemcmp" { rt.arg(2) } else { u32::MAX };
            let mut r = 0i32;
            for i in 0..n {
                let (x, y) = (rt.mem.r32(a + 4 * i) as i32, rt.mem.r32(b + 4 * i) as i32);
                if x != y || (name == "wcscoll" && x == 0) {
                    r = x.cmp(&y) as i32;
                    break;
                }
            }
            rt.ret(r as u32)
        }
        "wcsxfrm" => {
            let (d, src, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let len = (0..).find(|&i| rt.mem.r32(src + 4 * i) == 0).unwrap();
            if n > 0 {
                let keep = len.min(n - 1);
                for i in 0..keep {
                    let c = rt.mem.r32(src + 4 * i);
                    rt.mem.w32(d + 4 * i, c);
                }
                rt.mem.w32(d + 4 * keep, 0);
            }
            rt.ret(len)
        }
        "setlocale" => {
            let s = rt.alloc_str("C");
            rt.ret(s)
        }
        "qsort" => {
            let (base, n, size, cmp) = (rt.arg(0), rt.arg(1), rt.arg(2), rt.arg(3));
            // Insertion sort through the game's comparator, on two scratch elements.
            let (ta, tb) = (rt.malloc(size), rt.malloc(size));
            let mut items: Vec<Vec<u8>> = (0..n).map(|i| rt.mem.read(base + i * size, size as usize)).collect();
            'sort: for i in 1..items.len() {
                for j in (1..=i).rev() {
                    rt.mem.write(ta, &items[j - 1]);
                    rt.mem.write(tb, &items[j]);
                    match rt.call(cmp, &[ta, tb]) {
                        Ok(r) if (r as i32) > 0 => items.swap(j - 1, j),
                        Ok(_) => break,
                        Err(e) => {
                            rt.pending = Some(e);
                            break 'sort;
                        }
                    }
                }
            }
            for (k, it) in items.iter().enumerate() {
                rt.mem.write(base + k as u32 * size, it);
            }
            rt.free(ta);
            rt.free(tb);
            rt.ret(0)
        }
        "srand48" => {
            rt.state.rand = rt.arg(0) as u64;
            rt.ret(0)
        }
        "lrand48" => {
            rt.state.rand = rt.state.rand.wrapping_mul(0x5_DEEC_E66D).wrapping_add(0xB) & ((1 << 48) - 1);
            let v = (rt.state.rand >> 17) as u32;
            rt.ret(v)
        }

        // ---- setjmp/longjmp: r4-r14 and d8-d15 in the buffer
        "setjmp" | "_setjmp" | "sigsetjmp" => {
            let buf = rt.arg(0);
            let r = rt.cpu.r;
            for (k, n) in (4..=14).enumerate() {
                rt.mem.w32(buf + 4 * k as u32, r[n]);
            }
            if let Some(s) = rt.cpu.v7.as_ref().map(|v| v.s) {
                for k in 0..16u32 {
                    rt.mem.w32(buf + 44 + 4 * k, s[16 + k as usize]);
                }
            }
            rt.ret(0)
        }
        "longjmp" | "_longjmp" | "siglongjmp" => {
            let (buf, v) = (rt.arg(0), rt.arg(1));
            for (k, n) in (4..=14).enumerate() {
                rt.cpu.r[n] = rt.mem.r32(buf + 4 * k as u32);
            }
            let saved: Vec<u32> = (0..16).map(|k| rt.mem.r32(buf + 44 + 4 * k)).collect();
            if let Some(v7) = rt.cpu.v7.as_mut() {
                v7.s[16..32].copy_from_slice(&saved);
            }
            rt.ret(if v == 0 { 1 } else { v })
        }

        // ---- signals, scheduling, sleeping: nothing to do with one thread
        "sigaction" | "bsd_signal" | "raise" | "kill" | "pthread_sigmask" | "sigwait" | "pthread_attr_setscope"
        | "pthread_attr_setstackaddr" | "pthread_condattr_setpshared" | "pthread_kill" | "openlog" | "syslog" | "setvbuf" | "settimeofday" | "timer_delete"
        | "timer_settime" | "timer_gettime" | "flock" | "fsync" | "utime" | "chmod" | "chown" | "chdir" => rt.ret(0),
        "pthread_condattr_getpshared" | "pthread_getschedparam" | "sched_getparam" => {
            let out = rt.arg(1);
            rt.mem.w32(out, 0);
            rt.ret(0)
        }
        "sched_get_priority_max" => rt.ret(99),
        "sched_get_priority_min" => rt.ret(0),
        "timer_create" => {
            let out = rt.arg(2);
            rt.mem.w32(out, 1);
            rt.ret(0)
        }

        // ---- sockets and the rest: none (ENOENT). Files are in files.rs.
        "socket" | "popen" | "statfs" | "readlink" | "syscall" | "getaddrinfo" | "inet_aton" | "gethostname" => {
            rt.set_errno(2);
            rt.ret(if name == "popen" { 0 } else { u32::MAX })
        }
        "pclose" => rt.ret(0),
        "fscanf" => rt.ret(0),
        "getwc" | "ungetwc" | "ioctl" | "poll" | "select" => rt.ret(u32::MAX),
        "writev" => {
            let (iov, cnt) = (rt.arg(1), rt.arg(2));
            let total = (0..cnt).map(|i| rt.mem.r32(iov + 8 * i + 4)).sum::<u32>();
            rt.ret(total)
        }
        "fputc" | "putc" | "putwc" | "__swbuf" => {
            let (c, fp) = (rt.arg(0), rt.arg(1));
            if name != "putwc" {
                rt.stream_put(fp, &[c as u8]);
            }
            rt.ret(if name == "putwc" { c } else { c & 0xFF })
        }

        // ---- zlib: whole buffers, and inflate streams (PPAD2's fs.arc). A 32-bit z_stream:
        // next_in 0, avail_in 4, total_in 8, next_out 12, avail_out 16, total_out 20, msg 24,
        // state 28 ... adler 48.
        // inflateInit_(strm, version, stream_size)
        "inflateInit_" => {
            let z = rt.arg(0);
            for o in [8, 20, 24, 28] {
                rt.mem.w32(z + o, 0);
            }
            rt.mem.w32(z + 48, 1);
            rt.state.inflate.insert(z, miniz_oxide::inflate::stream::InflateState::new_boxed(miniz_oxide::DataFormat::Zlib));
            rt.ret(0)
        }
        // inflate(strm, flush): Z_OK, Z_STREAM_END, Z_NEED_DICT or an error (Z_STREAM_ERROR,
        // Z_DATA_ERROR, Z_BUF_ERROR)
        "inflate" => {
            let (z, flush) = (rt.arg(0), rt.arg(1));
            let (next_in, avail_in, next_out, avail_out) = (rt.mem.r32(z), rt.mem.r32(z + 4), rt.mem.r32(z + 12), rt.mem.r32(z + 16));
            let input = rt.mem.read(next_in, avail_in as usize);
            let mut output = vec![0; avail_out as usize];
            let (Some(state), Ok(flush)) = (rt.state.inflate.get_mut(&z), miniz_oxide::MZFlush::new(flush as i32)) else {
                rt.ret(-2i32 as u32);
                return Ok(());
            };
            let r = miniz_oxide::inflate::stream::inflate(state, &input, &mut output, flush);
            let adler = state.decompressor().adler32();
            let (used, made) = (r.bytes_consumed as u32, r.bytes_written as u32);
            rt.mem.write(next_out, &output[..made as usize]);
            rt.mem.w32(z, next_in + used);
            rt.mem.w32(z + 4, avail_in - used);
            let total_in = rt.mem.r32(z + 8);
            rt.mem.w32(z + 8, total_in + used);
            rt.mem.w32(z + 12, next_out + made);
            rt.mem.w32(z + 16, avail_out - made);
            let total_out = rt.mem.r32(z + 20);
            rt.mem.w32(z + 20, total_out + made);
            if let Some(a) = adler {
                rt.mem.w32(z + 48, a);
            }
            use miniz_oxide::{MZError, MZStatus};
            rt.ret(match r.status {
                Ok(MZStatus::Ok) => 0,
                Ok(MZStatus::StreamEnd) => 1,
                Ok(MZStatus::NeedDict) => 2,
                Err(MZError::Data) => -3i32 as u32,
                Err(MZError::Buf) => -5i32 as u32,
                Err(_) => -2i32 as u32,
            })
        }
        // inflateReset(strm), inflateEnd(strm)
        "inflateReset" => {
            let z = rt.arg(0);
            let r = match rt.state.inflate.get_mut(&z) {
                Some(state) => {
                    state.reset(miniz_oxide::DataFormat::Zlib);
                    rt.mem.w32(z + 8, 0);
                    rt.mem.w32(z + 20, 0);
                    0
                }
                None => -2i32 as u32,
            };
            rt.ret(r)
        }
        "inflateEnd" => {
            let z = rt.arg(0);
            let r = if rt.state.inflate.remove(&z).is_some() { 0 } else { -2i32 as u32 };
            rt.ret(r)
        }
        // uncompress(dest, uLongf* destLen, src, srcLen): Z_OK, Z_BUF_ERROR or Z_DATA_ERROR
        "uncompress" => {
            let (dest, len_at, src, n) = (rt.arg(0), rt.arg(1), rt.arg(2), rt.arg(3));
            let cap = rt.mem.r32(len_at) as usize;
            let input = rt.mem.read(src, n as usize);
            let r = match miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&input, cap) {
                Ok(out) => {
                    rt.mem.write(dest, &out);
                    rt.mem.w32(len_at, out.len() as u32);
                    0
                }
                Err(e) if e.status == miniz_oxide::inflate::TINFLStatus::HasMoreOutput => -5i32 as u32,
                Err(_) => -3i32 as u32,
            };
            rt.ret(r)
        }
        // crc32(crc, buf, len)
        "crc32" => {
            let (crc, buf, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let mut c = !crc;
            if buf != 0 {
                for b in rt.mem.read(buf, n as usize) {
                    c ^= b as u32;
                    for _ in 0..8 {
                        c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
                    }
                }
            }
            rt.ret(if buf == 0 { 0 } else { !c })
        }

        // ---- time of day (UTC)
        // strftime(buf, max, fmt, const struct tm*): the C locale's names, UTC
        "strftime" => {
            let (buf, max, f, tm) = (rt.arg(0), rt.arg(1), rt.arg(2), rt.arg(3));
            let f = rt.mem.cstr(f);
            let t: Vec<i32> = (0..8).map(|i| rt.mem.r32(tm + 4 * i) as i32).collect();
            let (sec, min, hour, mday, mon, year, wday, yday) = (t[0], t[1], t[2], t[3], t[4], t[5] + 1900, t[6], t[7]);
            const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
            const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
            let day = DAYS[wday.rem_euclid(7) as usize];
            let month = MONTHS[mon.rem_euclid(12) as usize];
            let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
            let mut out = String::new();
            let mut it = f.chars();
            while let Some(c) = it.next() {
                if c != '%' {
                    out.push(c);
                    continue;
                }
                let s = match it.next() {
                    Some('Y') => year.to_string(),
                    Some('y') => format!("{:02}", year % 100),
                    Some('m') => format!("{:02}", mon + 1),
                    Some('d') => format!("{mday:02}"),
                    Some('e') => format!("{mday:2}"),
                    Some('H') => format!("{hour:02}"),
                    Some('I') => format!("{h12:02}"),
                    Some('M') => format!("{min:02}"),
                    Some('S') => format!("{sec:02}"),
                    Some('j') => format!("{:03}", yday + 1),
                    Some('p') => if hour < 12 { "AM" } else { "PM" }.into(),
                    Some('a') => day[..3].into(),
                    Some('A') => day.into(),
                    Some('b' | 'h') => month[..3].into(),
                    Some('B') => month.into(),
                    Some('w') => wday.to_string(),
                    Some('D') => format!("{:02}/{mday:02}/{:02}", mon + 1, year % 100),
                    Some('F') => format!("{year}-{:02}-{mday:02}", mon + 1),
                    Some('T') => format!("{hour:02}:{min:02}:{sec:02}"),
                    Some('R') => format!("{hour:02}:{min:02}"),
                    Some('c') => format!("{} {} {mday:2} {hour:02}:{min:02}:{sec:02} {year}", &day[..3], &month[..3]),
                    Some('x') => format!("{:02}/{mday:02}/{:02}", mon + 1, year % 100),
                    Some('X') => format!("{hour:02}:{min:02}:{sec:02}"),
                    Some('Z') => "UTC".into(),
                    Some('z') => "+0000".into(),
                    Some('n') => "
".into(),
                    Some('t') => "	".into(),
                    Some('%') => "%".into(),
                    Some(o) => format!("%{o}"),
                    None => String::new(),
                };
                out.push_str(&s);
            }
            // 0 when it doesn't fit (with its NUL).
            if out.len() < max as usize {
                rt.mem.write(buf, out.as_bytes());
                rt.mem.w8(buf + out.len() as u32, 0);
                rt.ret(out.len() as u32)
            } else {
                rt.ret(0)
            }
        }
        "gmtime_r" | "localtime" | "localtime_r" => {
            let t = rt.arg(0);
            let secs = rt.mem.r32(t) as i32 as i64;
            let out = if name == "localtime" { rt.malloc(44) } else { rt.arg(1) };
            let days = secs.div_euclid(86_400);
            let rem = secs.rem_euclid(86_400);
            // Civil date from days (Howard Hinnant's algorithm).
            let z = days + 719_468;
            let era = z.div_euclid(146_097);
            let doe = z - era * 146_097;
            let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let d = doy - (153 * mp + 2) / 5 + 1;
            let m = if mp < 10 { mp + 3 } else { mp - 9 };
            let y = yoe + era * 400 + (m <= 2) as i64;
            let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
            let cum = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
            let yday = cum[(m - 1) as usize] + d - 1 + (leap && m > 2) as i64;
            let fields = [rem % 60, rem / 60 % 60, rem / 3600, d, m - 1, y - 1900, (days + 4).rem_euclid(7), yday, 0, 0];
            for (k, v) in fields.iter().enumerate() {
                rt.mem.w32(out + 4 * k as u32, *v as u32);
            }
            rt.ret(out)
        }
        "difftime" => {
            let (a, b) = (rt.arg(0) as i32, rt.arg(1) as i32);
            rt.ret64(((a - b) as f64).to_bits())
        }

        // ---- memory maps: anonymous ones come from the heap. One of an asset's descriptor
        // (`AAsset_openFileDescriptor`) is a copy of its bytes from `offset`: PPAD2 maps its
        // game-rio.rdt (30 MB) this way, and read zeros before, so found no fonts.
        // mmap(addr, length, prot, flags, fd, offset)
        "mmap" => {
            let (n, fd, offset) = (rt.arg(1), rt.arg(4), rt.arg(5));
            let a = rt.malloc(n.next_multiple_of(0x1000) + 0x1000).next_multiple_of(0x1000);
            if let Some(data) = rt.assets.fds.get(&fd) {
                let from = (offset as usize).min(data.len());
                let bytes = data[from..(from + n as usize).min(data.len())].to_vec();
                rt.mem.write(a, &bytes);
            }
            rt.ret(a)
        }
        "munmap" => rt.ret(0),

        // ---- libm (soft-float ABI: doubles in r0/r1, floats in r0)
        n if math(rt, n) => {}

        // ---- libandroid's asset manager
        n if n.starts_with("AAsset") => return crate::assets::call(rt, n),
        n if n.starts_with("ASensor") || n.starts_with("ALooper") || n.starts_with("ANativeWindow") => return crate::android::call(rt, n),

        _ => return Err(Stop::Import(name.to_string(), lr)),
    }
    Ok(())
}

/// libm, by name; false when `n` isn't one of these.
fn math(rt: &mut Runtime, n: &str) -> bool {
    let d1: Option<fn(f64) -> f64> = match n {
        "sin" => Some(f64::sin),
        "cos" => Some(f64::cos),
        "tan" => Some(f64::tan),
        "asin" => Some(f64::asin),
        "acos" => Some(f64::acos),
        "atan" => Some(f64::atan),
        "sqrt" => Some(f64::sqrt),
        "exp" => Some(f64::exp),
        "log" => Some(f64::ln),
        "log10" => Some(f64::log10),
        "floor" => Some(f64::floor),
        "ceil" => Some(f64::ceil),
        "fabs" => Some(f64::abs),
        "round" => Some(f64::round),
        "trunc" => Some(f64::trunc),
        "sinh" => Some(f64::sinh),
        "cosh" => Some(f64::cosh),
        "tanh" => Some(f64::tanh),
        _ => None,
    };
    if let Some(f) = d1 {
        let v = f64_arg(rt, 0);
        rt.ret64(f(v).to_bits());
        return true;
    }
    let f1: Option<fn(f32) -> f32> = match n {
        "sinf" => Some(f32::sin),
        "cosf" => Some(f32::cos),
        "tanf" => Some(f32::tan),
        "asinf" => Some(f32::asin),
        "acosf" => Some(f32::acos),
        "atanf" => Some(f32::atan),
        "sqrtf" => Some(f32::sqrt),
        "expf" => Some(f32::exp),
        "logf" => Some(f32::ln),
        "log10f" => Some(f32::log10),
        "floorf" => Some(f32::floor),
        "ceilf" => Some(f32::ceil),
        "fabsf" => Some(f32::abs),
        "roundf" => Some(f32::round),
        "truncf" => Some(f32::trunc),
        _ => None,
    };
    if let Some(f) = f1 {
        let v = f32_arg(rt, 0);
        rt.ret(f(v).to_bits());
        return true;
    }
    match n {
        "pow" | "atan2" | "fmod" | "hypot" | "fmin" | "fmax" => {
            let (a, b) = (f64_arg(rt, 0), f64_arg(rt, 2));
            let r = match n {
                "pow" => a.powf(b),
                "atan2" => a.atan2(b),
                "fmod" => a % b,
                "hypot" => a.hypot(b),
                "fmin" => a.min(b),
                _ => a.max(b),
            };
            rt.ret64(r.to_bits());
            true
        }
        "powf" | "atan2f" | "fmodf" | "hypotf" | "fminf" | "fmaxf" => {
            let (a, b) = (f32_arg(rt, 0), f32_arg(rt, 1));
            let r = match n {
                "powf" => a.powf(b),
                "atan2f" => a.atan2(b),
                "fmodf" => a % b,
                "hypotf" => a.hypot(b),
                "fminf" => a.min(b),
                _ => a.max(b),
            };
            rt.ret(r.to_bits());
            true
        }
        "ldexp" => {
            let (a, e) = (f64_arg(rt, 0), rt.arg(2) as i32);
            rt.ret64((a * 2f64.powi(e)).to_bits());
            true
        }
        "ldexpf" => {
            let (a, e) = (f32_arg(rt, 0), rt.arg(1) as i32);
            rt.ret((a * 2f32.powi(e)).to_bits());
            true
        }
        "lrint" | "lround" => {
            let a = f64_arg(rt, 0);
            rt.ret(a.round() as i32 as u32);
            true
        }
        "lrintf" | "lroundf" => {
            let a = f32_arg(rt, 0);
            rt.ret(a.round() as i32 as u32);
            true
        }
        _ => false,
    }
}

/// Functions Ribbit handed out (`Runtime::host_fn`).
pub fn host_call(rt: &mut Runtime, name: &str) -> Result<(), Stop> {
    if name.starts_with("sl:") || name.starts_with("xa:") {
        return crate::sles::call(rt, name);
    }
    if name == "vsync" {
        return crate::threads::call(rt, name);
    }
    Err(Stop::Import(format!("host:{name}"), rt.cpu.r[14]))
}

impl Runtime {
    /// Time since the runtime started (CLOCK_MONOTONIC).
    /// The monotonic clock: time since the first reading, plus time skipped while every
    /// thread waited.
    pub(crate) fn state_elapsed(&self) -> std::time::Duration {
        START.get_or_init(start).0.elapsed() + self.threads.skew
    }

    /// The real-time clock (since 1970), on the same time base.
    pub(crate) fn wall_clock(&self) -> std::time::Duration {
        START.get_or_init(start).1 + self.state_elapsed()
    }
}

static START: std::sync::OnceLock<(std::time::Instant, std::time::Duration)> = std::sync::OnceLock::new();

fn start() -> (std::time::Instant, std::time::Duration) {
    (std::time::Instant::now(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default())
}
