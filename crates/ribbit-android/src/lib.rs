//! Ribbit on Android: the shared frontend inside a NativeActivity.
//!
//! Settings and saves live in the app's private storage. To open a BIOS or
//! cartridge from anywhere (Downloads, SD card) the in-app file browser needs
//! "All files access", which Android only grants from its settings screen;
//! the app opens that screen once if the permission is missing.

#![cfg(target_os = "android")]

use jni::objects::JObject;
use std::path::PathBuf;
use std::sync::OnceLock;
use winit::event_loop::EventLoop;
use winit::platform::android::EventLoopBuilderExtAndroid;
use winit::platform::android::activity::AndroidApp;

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("ribbit"),
    );
    let data_dir = app.internal_data_path().unwrap_or_else(|| PathBuf::from("/data/local/tmp/ribbit"));
    if let Err(e) = request_all_files_access(&app) {
        log::warn!("could not check storage permission: {e}");
    }
    let _ = ACTIVITY.set((app.vm_as_ptr() as usize, app.activity_as_ptr() as usize));
    let event_loop = match EventLoop::builder().with_android_app(app).build() {
        Ok(l) => l,
        Err(e) => {
            log::error!("event loop: {e}");
            return;
        }
    };
    let opts = ribbit_frontend::StartOptions {
        data_dir,
        destroyed: Some(activity_destroyed),
        pick_start: Some(pick_start),
        pick_poll: Some(pick_poll),
        keep_screen_on: Some(keep_screen_on),
        second_display: Some(SECOND_DISPLAY),
        ..Default::default()
    };
    if let Err(e) = ribbit_frontend::run_with(event_loop, opts) {
        log::error!("ribbit stopped: {e:#}");
    }
    // Saved and stopped. The process goes too: Android may keep it for the
    // next start, where winit 0.30 cannot make a second event loop. `_exit`,
    // as the C exit handlers abort while other threads (sound, Discord) run.
    // SAFETY: ends the process; nothing is left to write.
    unsafe { libc::_exit(0) };
}

/// Keeps the screen on (or lets it sleep again): on while a game plays.
/// Done in Java on Android's UI thread (Screen.keepOn): android-activity's
/// own `set_window_flags` waits for state winit holds while Ribbit handles
/// an event, and never returned ("Ribbit isn't responding").
fn keep_screen_on(on: bool) {
    use jni::objects::JValue;
    with_activity(|env, activity| {
        let class = app_class(env, activity, "dev.ribbit.emulator.Screen")?;
        env.call_static_method(&class, "keepOn", "(Landroid/app/Activity;Z)V", &[JValue::Object(activity), JValue::Bool(on.into())])?;
        Ok(())
    });
}

/// EXPERIMENTAL: the device's second display (SecondScreen.java), where the
/// L-MAX's second picture goes on a dual-screen handheld. Not tried on one.
const SECOND_DISPLAY: ribbit_frontend::SecondDisplay = ribbit_frontend::SecondDisplay {
    show: second_show,
    generation: second_generation,
    acquire: second_acquire,
    release: second_release,
    touch: second_touch,
};

#[link(name = "android")]
unsafe extern "C" {
    fn ANativeWindow_fromSurface(env: *mut jni::sys::JNIEnv, surface: jni::sys::jobject) -> *mut std::ffi::c_void;
    fn ANativeWindow_release(window: *mut std::ffi::c_void);
}

fn second_screen<'a>(env: &mut jni::JNIEnv<'a>, activity: &JObject) -> jni::errors::Result<jni::objects::JClass<'a>> {
    app_class(env, activity, "dev.ribbit.emulator.SecondScreen")
}

fn second_show(on: bool) -> bool {
    use jni::objects::JValue;
    with_activity(|env, activity| {
        let class = second_screen(env, activity)?;
        env.call_static_method(&class, "show", "(Landroid/app/Activity;Z)Z", &[JValue::Object(activity), JValue::Bool(on.into())])?.z()
    })
    .unwrap_or(false)
}

fn second_generation() -> u32 {
    with_activity(|env, activity| {
        let class = second_screen(env, activity)?;
        env.call_static_method(&class, "generation", "()I", &[])?.i()
    })
    .unwrap_or(0) as u32
}

/// The second display's surface as an `ANativeWindow` (a reference of our
/// own, let go of in `second_release`), and its size.
fn second_acquire() -> Option<(std::ptr::NonNull<std::ffi::c_void>, u32, u32)> {
    with_activity(|env, activity| {
        let class = second_screen(env, activity)?;
        let surface = env.call_static_method(&class, "acquire", "()Landroid/view/Surface;", &[])?.l()?;
        if surface.is_null() {
            return Ok(None);
        }
        let w = env.call_static_method(&class, "width", "()I", &[])?.i()?;
        let h = env.call_static_method(&class, "height", "()I", &[])?.i()?;
        // SAFETY: a valid Surface (a local reference alive here).
        let window = unsafe { ANativeWindow_fromSurface(env.get_raw(), surface.as_raw()) };
        let Some(window) = std::ptr::NonNull::new(window) else {
            env.call_static_method(&class, "released", "()V", &[])?;
            return Ok(None);
        };
        Ok(Some((window, w.max(1) as u32, h.max(1) as u32)))
    })
    .flatten()
}

fn second_release(window: std::ptr::NonNull<std::ffi::c_void>) {
    // SAFETY: the reference `second_acquire` took.
    unsafe { ANativeWindow_release(window.as_ptr()) };
    with_activity(|env, activity| {
        let class = second_screen(env, activity)?;
        env.call_static_method(&class, "released", "()V", &[])?;
        Ok(())
    });
}

/// A finger on the second display, in its surface's pixels.
fn second_touch() -> Option<(f32, f32)> {
    with_activity(|env, activity| {
        let class = second_screen(env, activity)?;
        let at = env.call_static_method(&class, "touch", "()[F", &[])?.l()?;
        if at.is_null() {
            return Ok(None);
        }
        let mut xy = [0.0f32; 2];
        env.get_float_array_region(&jni::objects::JFloatArray::from(at), 0, &mut xy)?;
        Ok(Some((xy[0], xy[1])))
    })
    .flatten()
}

/// The Java VM and the activity, for `activity_destroyed`.
static ACTIVITY: OnceLock<(usize, usize)> = OnceLock::new();

/// Whether Android has destroyed the activity (`Activity.isDestroyed`, already
/// true when its native `onDestroy` runs).
fn activity_destroyed() -> bool {
    let Some(&(vm, activity)) = ACTIVITY.get() else {
        return false;
    };
    // SAFETY: the pointers come from the running NativeActivity; the activity
    // reference stays valid until `android_main` returns.
    let destroyed = || -> jni::errors::Result<bool> {
        let vm = unsafe { jni::JavaVM::from_raw(vm as *mut _) }?;
        let mut env = vm.get_env()?;
        let activity = unsafe { JObject::from_raw(activity as jni::sys::jobject) };
        env.call_method(&activity, "isDestroyed", "()Z", &[])?.z()
    };
    destroyed().unwrap_or(false)
}

/// Runs `f` with the JNI environment and the activity, in a local frame of
/// its own (this thread runs for the app's life: references would pile up);
/// None if it fails (a Java exception is cleared).
fn with_activity<T>(f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> jni::errors::Result<T>) -> Option<T> {
    let &(vm, activity) = ACTIVITY.get()?;
    // SAFETY: the pointers come from the running NativeActivity.
    let vm = unsafe { jni::JavaVM::from_raw(vm as *mut _) }.ok()?;
    let mut env = vm.get_env().ok()?;
    let activity = unsafe { JObject::from_raw(activity as jni::sys::jobject) };
    let result = env.with_local_frame(16, |env| f(env, &activity));
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    result.map_err(|e| log::warn!("JNI: {e}")).ok()
}

/// The app's PickerActivity (Android's file picker).
fn picker<'a>(env: &mut jni::JNIEnv<'a>, activity: &JObject) -> jni::errors::Result<jni::objects::JClass<'a>> {
    app_class(env, activity, "dev.ribbit.emulator.PickerActivity")
}

/// One of the app's own Java classes, loaded by the app's class loader (this
/// native thread's own only knows Android's classes).
fn app_class<'a>(env: &mut jni::JNIEnv<'a>, activity: &JObject, name: &str) -> jni::errors::Result<jni::objects::JClass<'a>> {
    use jni::objects::JValue;
    let loader = env.call_method(activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
    let name = env.new_string(name)?;
    let class = env.call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&name)])?.l()?;
    Ok(class.into())
}

/// Opens Android's file picker for a file (or a folder); false if it can't.
fn pick_start(folder: bool) -> bool {
    use jni::objects::JValue;
    with_activity(|env, activity| {
        let class = picker(env, activity)?;
        env.call_static_method(&class, "start", "(Landroid/app/Activity;Z)V", &[JValue::Object(activity), JValue::Bool(folder.into())])?;
        Ok(())
    })
    .is_some()
}

/// The picker's answer once it is in: the chosen path, None if cancelled, or
/// why it can't be used.
fn pick_poll() -> Option<Result<Option<PathBuf>, String>> {
    with_activity(|env, activity| {
        let class = picker(env, activity)?;
        let answer = env.call_static_method(&class, "poll", "()Ljava/lang/String;", &[])?.l()?;
        if answer.is_null() {
            return Ok(None);
        }
        let answer: String = env.get_string(&answer.into())?.into();
        Ok(Some(match answer.strip_prefix('!') {
            _ if answer.is_empty() => Ok(None),
            Some(why) => Err(why.to_string()),
            None => Ok(Some(PathBuf::from(answer))),
        }))
    })
    .flatten()
}

/// Opens Android's "All files access" screen for Ribbit if not yet granted
/// (Android 11+; older versions use the manifest's storage permission).
fn request_all_files_access(app: &AndroidApp) -> jni::errors::Result<()> {
    use jni::objects::{JObject, JValue};
    // SAFETY: the pointers come from the running NativeActivity.
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as *mut _) }?;
    let mut env = vm.attach_current_thread()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
    let sdk = env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()?;
    if sdk < 30 {
        return Ok(());
    }
    let granted = env.call_static_method("android/os/Environment", "isExternalStorageManager", "()Z", &[])?.z()?;
    if granted {
        return Ok(());
    }
    let package = env.call_method(&activity, "getPackageName", "()Ljava/lang/String;", &[])?.l()?;
    let package: String = env.get_string((&package).into())?.into();
    let uri_text = env.new_string(format!("package:{package}"))?;
    let uri = env
        .call_static_method(
            "android/net/Uri",
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&uri_text)],
        )?
        .l()?;
    let action = env.new_string("android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION")?;
    let intent = env.new_object(
        "android/content/Intent",
        "(Ljava/lang/String;Landroid/net/Uri;)V",
        &[JValue::Object(&action), JValue::Object(&uri)],
    )?;
    env.call_method(&activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
    log::info!("asked for All files access");
    Ok(())
}
