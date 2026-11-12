//! Save states of the HLE: the processes, their threads and files, and the
//! machine under them. The file tree goes in as its changes from the one it
//! was built from (`base`: the system and the cartridge, as the app builds
//! them again). Times go in from the programs' clock at saving and come
//! back from the clock at loading, so every deadline is as far off as it
//! was. Pipes and message queues shared by several files or threads stay
//! shared: each is written once, then by number.

use super::Process;
use super::vfs::{Node, Vfs};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// "HLE" and a version: the first word of a state (Original's states start
/// with their version, a small number, so neither loads the other's). 2:
/// `mem::Frames`/`Mem` gained fields (shared pages, the Explorer's pool) and
/// the /dev/dsp driver changed, so a version 1 state can't be read.
const MAGIC: u32 = 0x454C_4802;

thread_local! {
    /// The programs' clock when saving or loading: times go in from it.
    static NOW: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Shared values written so far, by address → number.
    static WRITTEN: RefCell<HashMap<usize, u32>> = RefCell::new(HashMap::new());
    /// Shared values read so far, by number.
    static READ: RefCell<HashMap<u32, Box<dyn Any>>> = RefCell::new(HashMap::new());
}

/// Sets the clock for (de)serialising; clears it and the shared values after.
struct Session;

impl Session {
    fn new(now: Instant) -> Self {
        NOW.set(Some(now));
        Self
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        NOW.set(None);
        WRITTEN.with(|w| w.borrow_mut().clear());
        READ.with(|r| r.borrow_mut().clear());
    }
}

pub mod inst {
    use super::*;

    /// Nanoseconds from the clock (before it: negative).
    pub fn to(t: Instant) -> i64 {
        let now = NOW.get().expect("an HLE state session");
        match t.checked_duration_since(now) {
            Some(d) => d.as_nanos() as i64,
            None => -(now.duration_since(t).as_nanos() as i64),
        }
    }

    pub fn from(n: i64) -> Instant {
        let now = NOW.get().expect("an HLE state session");
        let d = std::time::Duration::from_nanos(n.unsigned_abs());
        if n >= 0 { now + d } else { now.checked_sub(d).unwrap_or(now) }
    }

    pub fn serialize<S: Serializer>(t: &Instant, s: S) -> Result<S::Ok, S::Error> {
        to(*t).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Instant, D::Error> {
        i64::deserialize(d).map(from)
    }
}

pub mod inst_opt {
    use super::*;

    pub fn serialize<S: Serializer>(t: &Option<Instant>, s: S) -> Result<S::Ok, S::Error> {
        t.map(inst::to).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Instant>, D::Error> {
        Ok(Option::<i64>::deserialize(d)?.map(inst::from))
    }
}

pub mod inst_map {
    use super::*;

    pub fn serialize<S: Serializer>(m: &HashMap<u32, Instant>, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(m.iter().map(|(k, t)| (*k, inst::to(*t))))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<HashMap<u32, Instant>, D::Error> {
        Ok(Vec::<(u32, i64)>::deserialize(d)?.into_iter().map(|(k, n)| (k, inst::from(n))).collect())
    }
}

pub mod shared {
    use super::*;

    /// Its number, and the value the first time.
    pub fn serialize<T: Serialize, S: Serializer>(a: &Arc<Mutex<T>>, s: S) -> Result<S::Ok, S::Error> {
        let key = Arc::as_ptr(a) as usize;
        let (id, first) = WRITTEN.with(|w| {
            let mut w = w.borrow_mut();
            let n = w.len() as u32;
            match w.get(&key) {
                Some(&id) => (id, false),
                None => {
                    w.insert(key, n);
                    (n, true)
                }
            }
        });
        if first { (id, Some(&*a.lock().unwrap())).serialize(s) } else { (id, None::<&T>).serialize(s) }
    }

    pub fn deserialize<'de, T: Deserialize<'de> + 'static, D: Deserializer<'de>>(d: D) -> Result<Arc<Mutex<T>>, D::Error> {
        let (id, v) = <(u32, Option<T>)>::deserialize(d)?;
        READ.with(|r| {
            let mut r = r.borrow_mut();
            match v {
                Some(v) => {
                    let a = Arc::new(Mutex::new(v));
                    r.insert(id, Box::new(a.clone()));
                    Ok(a)
                }
                None => r
                    .get(&id)
                    .and_then(|b| b.downcast_ref::<Arc<Mutex<T>>>())
                    .cloned()
                    .ok_or_else(|| D::Error::custom("a shared value before its first")),
            }
        })
    }
}

pub mod shared_map {
    use super::*;

    struct One<'a, T>(&'a Arc<Mutex<T>>);

    impl<T: Serialize> Serialize for One<'_, T> {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            shared::serialize(self.0, s)
        }
    }

    struct Owned<T>(Arc<Mutex<T>>);

    impl<'de, T: Deserialize<'de> + 'static> Deserialize<'de> for Owned<T> {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            shared::deserialize(d).map(Owned)
        }
    }

    pub fn serialize<T: Serialize, S: Serializer>(m: &HashMap<String, Arc<Mutex<T>>>, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(m.iter().map(|(k, v)| (k, One(v))))
    }

    pub fn deserialize<'de, T: Deserialize<'de> + 'static, D: Deserializer<'de>>(d: D) -> Result<HashMap<String, Arc<Mutex<T>>>, D::Error> {
        Ok(Vec::<(String, Owned<T>)>::deserialize(d)?.into_iter().map(|(k, v)| (k, v.0)).collect())
    }
}

/// The file tree as changes from another.
#[derive(Serialize, Deserialize)]
pub struct Changes {
    nodes: Vec<(String, Node)>,
    removed: Vec<String>,
    next_ino: u32,
}

impl Vfs {
    pub fn changes_from(&self, base: &Vfs) -> Changes {
        Changes {
            nodes: self.nodes.iter().filter(|(p, n)| base.nodes.get(*p) != Some(n)).map(|(p, n)| (p.clone(), n.clone())).collect(),
            removed: base.nodes.keys().filter(|p| !self.nodes.contains_key(*p)).cloned().collect(),
            next_ino: self.next_ino,
        }
    }

    pub fn with_changes(base: &Vfs, c: Changes) -> Vfs {
        let mut v = base.clone();
        for p in &c.removed {
            v.nodes.remove(p);
        }
        v.nodes.extend(c.nodes);
        v.next_ino = c.next_ino;
        v
    }
}

impl Process {
    /// The programs and the machine now (a save state), the file tree as
    /// its changes from `base`.
    pub fn save_state(&self, base: &Vfs) -> Vec<u8> {
        let _s = Session::new(self.clock());
        // Straight into the buffer: bincode::serialize measures first, a
        // pass that would mark every shared value written.
        let mut data = Vec::new();
        bincode::serialize_into(&mut data, &(self, self.vfs.changes_from(base))).expect("serialise the HLE");
        let mut out = MAGIC.to_le_bytes().to_vec();
        out.extend(miniz_oxide::deflate::compress_to_vec(&data, 1));
        out
    }

    /// Restores a state from `save_state` (`base` the same tree as then).
    /// The JIT starts empty; the HD and 3D chip settings and whether the
    /// app has the programs paused are kept.
    pub fn load_state(&mut self, state: &[u8], base: &Vfs) -> Result<(), String> {
        if state.get(..4) != Some(&MAGIC.to_le_bytes()[..]) {
            if state.get(1..4) == Some(&MAGIC.to_le_bytes()[1..]) {
                return Err("this state is from another version of Enhanced (new)".into());
            }
            return Err("this state isn't from Enhanced (new)".into());
        }
        let data = miniz_oxide::inflate::decompress_to_vec(&state[4..]).map_err(|_| "a damaged Didj state".to_string())?;
        let now = Instant::now();
        let (mut p, changes): (Process, Changes) = {
            let _s = Session::new(now);
            bincode::deserialize(&data).map_err(|e| e.to_string())?
        };
        p.vfs = Vfs::with_changes(base, changes);
        p.paused_at = self.paused_at.map(|_| now);
        let m = &mut p.m;
        m.jit = crate::machine::jit_for(m.bus.ram_base, m.bus.sdram.len());
        super::set_up_jit(m);
        m.bus.nand = std::mem::take(&mut self.m.bus.nand);
        m.bus.flips = self.m.bus.flips;
        let (old, new) = (&mut self.m.bus.ga3d, &mut m.bus.ga3d);
        new.speed_up = old.speed_up;
        new.hd_textures = old.hd_textures;
        std::mem::swap(&mut new.hd, &mut old.hd);
        m.exact = self.m.exact;
        m.set_cpu_milli(self.m.cpu_milli());
        m.fonts = std::mem::take(&mut self.m.fonts);
        // A TLB flush before the first instruction.
        p.tlb_for = u32::MAX;
        p.trace = self.trace;
        *self = p;
        Ok(())
    }
}
