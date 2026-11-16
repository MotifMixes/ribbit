//! stdio streams made by `funopen`: the titles' own file layer opens an asset and wraps it in
//! a FILE with its read, seek and close functions, which are called back here. Other FILEs
//! (stdin, stdout, stderr) only take writes, which go to the log.
//!
//! bionic's `getc` and `feof` can be macros that read the FILE: `_p`/`_r` (offsets 0 and 4)
//! are left 0 so `getc` always calls `__srget`, and `_flags` (offset 12) carries __SEOF and
//! __SERR.

use crate::runtime::{Runtime, Stop};

/// bionic's FILE is 84 bytes.
const FILE_SIZE: u32 = 84;
const SEOF: u16 = 0x0020;
const SERR: u16 = 0x0040;
const EOF: u32 = u32::MAX;

pub struct Stream {
    cookie: u32,
    read: u32,
    write: u32,
    seek: u32,
    close: u32,
    eof: bool,
    err: bool,
    /// Bytes pushed back by `ungetc`, the next to read last.
    unget: Vec<u8>,
}

pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "funopen"
            | "fread"
            | "fwrite"
            | "fseek"
            | "fseeko"
            | "ftell"
            | "ftello"
            | "fclose"
            | "feof"
            | "ferror"
            | "clearerr"
            | "rewind"
            | "fgetc"
            | "getc"
            | "__srget"
            | "ungetc"
            | "fgets"
            | "fgetpos"
            | "fsetpos"
            | "fflush"
    )
}

impl Runtime {
    fn set_flags(&mut self, fp: u32) {
        let Some(s) = self.streams.get(&fp) else { return };
        let flags = if s.eof { SEOF } else { 0 } | if s.err { SERR } else { 0 };
        self.mem.w16(fp + 12, flags);
    }

    /// Reads up to `n` bytes into `buf` through the stream's read function; the count read.
    fn stream_read(&mut self, fp: u32, buf: u32, n: u32) -> Result<u32, Stop> {
        let mut done = 0;
        while done < n {
            let Some(s) = self.streams.get_mut(&fp) else { break };
            if let Some(b) = s.unget.pop() {
                self.mem.w8(buf + done, b);
                done += 1;
                continue;
            }
            let (cookie, read) = (s.cookie, s.read);
            if read == 0 {
                s.err = true;
                break;
            }
            let r = self.call(read, &[cookie, buf + done, n - done])? as i32;
            let s = self.streams.get_mut(&fp).unwrap();
            if r < 0 {
                s.err = true;
                break;
            }
            if r == 0 {
                s.eof = true;
                break;
            }
            done += r as u32;
        }
        self.set_flags(fp);
        Ok(done)
    }

    /// The stream's seek function; the new position, or -1.
    fn stream_seek(&mut self, fp: u32, off: u32, whence: u32) -> Result<u32, Stop> {
        let Some(s) = self.streams.get_mut(&fp) else { return Ok(EOF) };
        let (cookie, seek) = (s.cookie, s.seek);
        // A relative seek counts from where the reader is, before the pushed-back bytes.
        let off = if whence == 1 { off.wrapping_sub(s.unget.len() as u32) } else { off };
        s.unget.clear();
        if seek == 0 {
            return Ok(EOF);
        }
        let r = self.call(seek, &[cookie, off, whence])?;
        if r != EOF {
            if let Some(s) = self.streams.get_mut(&fp) {
                s.eof = false;
            }
            self.set_flags(fp);
        }
        Ok(r)
    }

    fn stream_tell(&mut self, fp: u32) -> Result<u32, Stop> {
        let Some(s) = self.streams.get(&fp) else { return Ok(EOF) };
        let (cookie, seek, back) = (s.cookie, s.seek, s.unget.len() as u32);
        if seek == 0 {
            return Ok(EOF);
        }
        let r = self.call(seek, &[cookie, 0, 1])?;
        Ok(if r == EOF { EOF } else { r - back })
    }

    fn stream_getc(&mut self, fp: u32) -> Result<u32, Stop> {
        let b = self.malloc(4);
        let n = self.stream_read(fp, b, 1)?;
        let c = self.mem.r8(b);
        self.free(b);
        Ok(if n == 1 { c as u32 } else { EOF })
    }
}

pub fn call(rt: &mut Runtime, name: &str) -> Result<(), Stop> {
    match name {
        // funopen(cookie, readfn, writefn, seekfn, closefn)
        "funopen" => {
            let (cookie, read, write, seek, close) = (rt.arg(0), rt.arg(1), rt.arg(2), rt.arg(3), rt.arg(4));
            let fp = rt.malloc(FILE_SIZE);
            rt.mem.write(fp, &[0; FILE_SIZE as usize]);
            rt.streams.insert(fp, Stream { cookie, read, write, seek, close, eof: false, err: false, unget: Vec::new() });
            rt.ret(fp)
        }
        // fread(ptr, size, nmemb, fp): whole items read
        "fread" => {
            let (buf, size, n, fp) = (rt.arg(0), rt.arg(1), rt.arg(2), rt.arg(3));
            let total = size.saturating_mul(n);
            if size == 0 || !rt.streams.contains_key(&fp) {
                rt.ret(0);
                return Ok(());
            }
            let got = rt.stream_read(fp, buf, total)?;
            rt.ret(got / size)
        }
        // fwrite(ptr, size, nmemb, fp): to the stream's write function, or the log
        "fwrite" => {
            let (buf, size, n, fp) = (rt.arg(0), rt.arg(1), rt.arg(2), rt.arg(3));
            let total = size.saturating_mul(n);
            match rt.streams.get(&fp).map(|s| (s.cookie, s.write)) {
                Some((cookie, write)) if write != 0 => {
                    let r = rt.call(write, &[cookie, buf, total])? as i32;
                    rt.ret(if r > 0 && size != 0 { r as u32 / size } else { 0 })
                }
                Some(_) => rt.ret(0),
                None => {
                    let text = String::from_utf8_lossy(&rt.mem.read(buf, total as usize)).into_owned();
                    crate::imports::log_line(rt, text);
                    rt.ret(n)
                }
            }
        }
        "fseek" | "fseeko" => {
            let (fp, off, whence) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let r = rt.stream_seek(fp, off, whence)?;
            rt.ret(if r == EOF { EOF } else { 0 })
        }
        "ftell" | "ftello" => {
            let fp = rt.arg(0);
            let r = rt.stream_tell(fp)?;
            rt.ret(r)
        }
        "rewind" => {
            let fp = rt.arg(0);
            rt.stream_seek(fp, 0, 0)?;
            if let Some(s) = rt.streams.get_mut(&fp) {
                s.err = false;
            }
            rt.set_flags(fp);
            rt.ret(0)
        }
        // fgetpos(fp, fpos_t*), fsetpos(fp, const fpos_t*): fpos_t is a 32-bit offset
        "fgetpos" => {
            let (fp, out) = (rt.arg(0), rt.arg(1));
            let p = rt.stream_tell(fp)?;
            if p == EOF {
                rt.ret(EOF);
            } else {
                rt.mem.w32(out, p);
                rt.ret(0)
            }
        }
        "fsetpos" => {
            let (fp, at) = (rt.arg(0), rt.arg(1));
            let off = rt.mem.r32(at);
            let r = rt.stream_seek(fp, off, 0)?;
            rt.ret(if r == EOF { EOF } else { 0 })
        }
        "fclose" => {
            let fp = rt.arg(0);
            match rt.streams.remove(&fp) {
                Some(s) => {
                    let r = if s.close != 0 { rt.call(s.close, &[s.cookie])? } else { 0 };
                    rt.free(fp);
                    rt.ret(if r == 0 { 0 } else { EOF })
                }
                None => rt.ret(0),
            }
        }
        "feof" | "ferror" => {
            let fp = rt.arg(0);
            let v = rt.streams.get(&fp).is_some_and(|s| if name == "feof" { s.eof } else { s.err });
            rt.ret(v as u32)
        }
        "clearerr" => {
            let fp = rt.arg(0);
            if let Some(s) = rt.streams.get_mut(&fp) {
                s.eof = false;
                s.err = false;
            }
            rt.set_flags(fp);
            rt.ret(0)
        }
        "fgetc" | "getc" | "__srget" => {
            let fp = rt.arg(0);
            let c = rt.stream_getc(fp)?;
            rt.ret(c)
        }
        // ungetc(c, fp)
        "ungetc" => {
            let (c, fp) = (rt.arg(0), rt.arg(1));
            match rt.streams.get_mut(&fp) {
                Some(s) if c != EOF => {
                    s.unget.push(c as u8);
                    s.eof = false;
                    rt.set_flags(fp);
                    rt.ret(c & 0xFF)
                }
                _ => rt.ret(EOF),
            }
        }
        // fgets(buf, n, fp): a line (with its newline), NUL-terminated; NULL at once at the end
        "fgets" => {
            let (buf, n, fp) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let mut len = 0;
            while len + 1 < n {
                let c = rt.stream_getc(fp)?;
                if c == EOF {
                    break;
                }
                rt.mem.w8(buf + len, c as u8);
                len += 1;
                if c == b'\n' as u32 {
                    break;
                }
            }
            if n > 0 {
                rt.mem.w8(buf + len, 0);
            }
            rt.ret(if len == 0 { 0 } else { buf })
        }
        "fflush" => rt.ret(0),
        _ => return Err(Stop::Import(name.to_string(), rt.cpu.r[14])),
    }
    Ok(())
}
