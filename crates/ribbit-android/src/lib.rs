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

/// The app's PickerActivity (Android's file picker), loaded by the app's
/// class loader (this native thread's own only knows Android's classes).
fn picker<'a>(env: &mut jni::JNIEnv<'a>, activity: &JObject) -> jni::errors::Result<jni::objects::JClass<'a>> {
    use jni::objects::JValue;
    let loader = env.call_method(activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
    let name = env.new_string("dev.ribbit.emulator.PickerActivity")?;
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
