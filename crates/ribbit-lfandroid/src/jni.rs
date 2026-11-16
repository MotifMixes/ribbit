//! A stand-in Java VM: the JavaVM and JNIEnv function tables the library calls through (JNI
//! 1.6, the table order of jni.h), with Java objects as handles. There is no Java: classes,
//! methods and fields are names the game asked for, method calls are logged and answer zero
//! or an empty object of the return type. `RegisterNatives` is what matters: it tells Ribbit
//! where the `na*` natives the Java shell calls are.

use crate::runtime::{JNI_ENV_FUNCS, JNI_VM_FUNCS, Native, Runtime, Stop};

const ENV_FUNCS: usize = 233;
const VM_FUNCS: usize = 8;
/// Handles are this plus an index into `Jni::objects` (never valid memory addresses).
const HANDLE: u32 = 0x0100_0000;

#[derive(Debug, Clone)]
pub enum Obj {
    Class(String),
    Object(String),
    Str(String),
    Method { class: String, name: String, sig: String },
    Field { class: String, name: String, sig: String },
    Array(u32),
}

#[derive(Default)]
pub struct Jni {
    /// The JavaVM* and JNIEnv* handed to the game.
    pub vm: u32,
    pub env: u32,
    pub objects: Vec<Obj>,
}

impl Jni {
    pub fn new_handle(&mut self, o: Obj) -> u32 {
        self.objects.push(o);
        HANDLE + self.objects.len() as u32 - 1
    }
    pub fn get(&self, h: u32) -> Option<&Obj> {
        self.objects.get(h.wrapping_sub(HANDLE) as usize)
    }
    fn class_of(&self, h: u32) -> String {
        match self.get(h) {
            Some(Obj::Class(c) | Obj::Object(c)) => c.clone(),
            Some(Obj::Str(_)) => "java/lang/String".into(),
            _ => "?".into(),
        }
    }
}

/// Builds the two tables and the JavaVM/JNIEnv structures (each a pointer to its table).
pub fn install(rt: &mut Runtime) {
    let env_table = rt.malloc(4 * ENV_FUNCS as u32);
    for i in 0..ENV_FUNCS as u32 {
        rt.mem.w32(env_table + 4 * i, JNI_ENV_FUNCS + 8 * i);
    }
    let vm_table = rt.malloc(4 * VM_FUNCS as u32);
    for i in 0..VM_FUNCS as u32 {
        rt.mem.w32(vm_table + 4 * i, JNI_VM_FUNCS + 8 * i);
    }
    rt.jni.env = rt.malloc(4);
    rt.mem.w32(rt.jni.env, env_table);
    rt.jni.vm = rt.malloc(4);
    rt.mem.w32(rt.jni.vm, vm_table);
}

/// The JavaVM's invoke interface.
pub fn vm_call(rt: &mut Runtime, i: usize) -> Result<(), Stop> {
    match i {
        // DestroyJavaVM, DetachCurrentThread
        3 | 5 => rt.ret(0),
        // AttachCurrentThread(vm, JNIEnv**, args), GetEnv(vm, void**, version),
        // AttachCurrentThreadAsDaemon
        4 | 6 | 7 => {
            let out = rt.arg(1);
            let env = rt.jni.env;
            rt.mem.w32(out, env);
            rt.ret(0)
        }
        _ => return Err(Stop::Jni("JavaVM", i, rt.cpu.r[14])),
    }
    Ok(())
}

/// What a method of this signature returns, as an answer: 0, or a new empty object.
fn answer_for(rt: &mut Runtime, sig: &str) -> u32 {
    let ret = sig.rsplit(')').next().unwrap_or("V");
    if ret == "Ljava/lang/String;" {
        rt.jni.new_handle(Obj::Str(String::new()))
    } else if let Some(c) = ret.strip_prefix('L').and_then(|r| r.strip_suffix(';')) {
        rt.jni.new_handle(Obj::Object(c.to_string()))
    } else {
        0
    }
}

pub fn env_call(rt: &mut Runtime, i: usize) -> Result<(), Stop> {
    let lr = rt.cpu.r[14];
    match i {
        // GetVersion: JNI 1.6
        4 => rt.ret(0x0001_0006),
        // FindClass(env, name)
        6 => {
            let a = rt.arg(1);
            let name = rt.mem.cstr(a);
            let h = rt.jni.new_handle(Obj::Class(name));
            rt.ret(h)
        }
        // GetSuperclass
        10 => {
            let h = rt.jni.new_handle(Obj::Class("java/lang/Object".into()));
            rt.ret(h)
        }
        // IsAssignableFrom, IsInstanceOf, IsSameObject (compare handles)
        11 | 32 => rt.ret(1),
        24 => {
            let same = rt.arg(1) == rt.arg(2);
            rt.ret(same as u32)
        }
        // Throw, ThrowNew: logged
        13 | 14 => {
            let msg = if i == 14 { rt.arg(2) } else { 0 };
            let m = if msg != 0 { rt.mem.cstr(msg) } else { String::new() };
            rt.log.push(format!("JNI throw {m}"));
            rt.ret(0)
        }
        // ExceptionOccurred: none; ExceptionDescribe, ExceptionClear
        15 | 16 | 17 => rt.ret(0),
        18 => {
            let a = rt.arg(1);
            let m = rt.mem.cstr(a);
            return Err(Stop::Halt(format!("JNI FatalError: {m}")));
        }
        // PushLocalFrame, EnsureLocalCapacity: OK; PopLocalFrame(env, result): result
        19 | 26 => rt.ret(0),
        20 => {
            let r = rt.arg(1);
            rt.ret(r)
        }
        // NewGlobalRef, NewLocalRef, NewWeakGlobalRef: the same handle
        21 | 25 | 226 => {
            let h = rt.arg(1);
            rt.ret(h)
        }
        // DeleteGlobalRef, DeleteLocalRef, DeleteWeakGlobalRef
        22 | 23 | 227 => rt.ret(0),
        // AllocObject(env, class), NewObject/V/A(env, class, ctor, ...)
        27..=30 => {
            let c = rt.arg(1);
            let class = rt.jni.class_of(c);
            let h = rt.jni.new_handle(Obj::Object(class));
            rt.ret(h)
        }
        // GetObjectClass
        31 => {
            let o = rt.arg(1);
            let class = rt.jni.class_of(o);
            let h = rt.jni.new_handle(Obj::Class(class));
            rt.ret(h)
        }
        // GetMethodID, GetStaticMethodID, GetFieldID, GetStaticFieldID (env, class, name, sig)
        33 | 113 | 94 | 144 => {
            let (c, n, s) = (rt.arg(1), rt.arg(2), rt.arg(3));
            let class = rt.jni.class_of(c);
            let (name, sig) = (rt.mem.cstr(n), rt.mem.cstr(s));
            let o = if i == 33 || i == 113 { Obj::Method { class, name, sig } } else { Obj::Field { class, name, sig } };
            let h = rt.jni.new_handle(o);
            rt.ret(h)
        }
        // Call<T>Method/V/A, CallNonvirtual<T>Method/V/A, CallStatic<T>Method/V/A
        34..=93 | 114..=143 => {
            let mid = if (64..=93).contains(&i) { rt.arg(3) } else { rt.arg(2) };
            let (desc, sig) = match rt.jni.get(mid) {
                Some(Obj::Method { class, name, sig }) => (format!("{class}.{name}{sig}"), sig.clone()),
                _ => (format!("method {mid:#x}"), "()V".into()),
            };
            rt.log.push(format!("JNI call {desc}"));
            let v = answer_for(rt, &sig);
            // Long and Double return in r0/r1.
            rt.cpu.r[1] = 0;
            rt.ret(v)
        }
        // Get<T>Field, GetStatic<T>Field: zero or an empty object
        95..=103 | 145..=153 => {
            let fid = rt.arg(2);
            let sig = match rt.jni.get(fid) {
                Some(Obj::Field { sig, .. }) => format!("(){sig}"),
                _ => "()I".into(),
            };
            let v = answer_for(rt, &sig);
            rt.cpu.r[1] = 0;
            rt.ret(v)
        }
        // Set<T>Field, SetStatic<T>Field: ignored
        104..=112 | 154..=162 => rt.ret(0),
        // NewStringUTF(env, chars)
        167 => {
            let a = rt.arg(1);
            let s = rt.mem.cstr(a);
            let h = rt.jni.new_handle(Obj::Str(s));
            rt.ret(h)
        }
        // GetStringUTFLength, GetStringLength
        164 | 168 => {
            let s = match { let h = rt.arg(1); rt.jni.get(h) } {
                Some(Obj::Str(s)) => s.clone(),
                _ => String::new(),
            };
            rt.ret(if i == 164 { s.encode_utf16().count() as u32 } else { s.len() as u32 })
        }
        // GetStringUTFChars(env, str, isCopy*)
        169 => {
            let s = match { let h = rt.arg(1); rt.jni.get(h) } {
                Some(Obj::Str(s)) => s.clone(),
                _ => String::new(),
            };
            let copy = rt.arg(2);
            if copy != 0 {
                rt.mem.w8(copy, 1);
            }
            let a = rt.alloc_str(&s);
            rt.ret(a)
        }
        // ReleaseStringUTFChars(env, str, chars), ReleaseStringChars
        166 | 170 => {
            let a = rt.arg(2);
            rt.free(a);
            rt.ret(0)
        }
        // GetArrayLength
        171 => {
            let n = match { let h = rt.arg(1); rt.jni.get(h) } {
                Some(Obj::Array(n)) => *n,
                _ => 0,
            };
            rt.ret(n)
        }
        // RegisterNatives(env, class, methods, n): {name, signature, fnPtr} each
        215 => {
            let (c, m, n) = (rt.arg(1), rt.arg(2), rt.arg(3));
            let class = rt.jni.class_of(c);
            for k in 0..n {
                let e = m + 12 * k;
                let (na, sa, f) = (rt.mem.r32(e), rt.mem.r32(e + 4), rt.mem.r32(e + 8));
                let (name, signature) = (rt.mem.cstr(na), rt.mem.cstr(sa));
                rt.natives.insert(name, Native { class: class.clone(), signature, addr: f });
            }
            rt.ret(0)
        }
        // UnregisterNatives, MonitorEnter, MonitorExit
        216..=218 => rt.ret(0),
        // GetJavaVM(env, JavaVM**)
        219 => {
            let out = rt.arg(1);
            let vm = rt.jni.vm;
            rt.mem.w32(out, vm);
            rt.ret(0)
        }
        // ExceptionCheck: no exception
        228 => rt.ret(0),
        _ => return Err(Stop::Jni("JNIEnv", i, lr)),
    }
    Ok(())
}
