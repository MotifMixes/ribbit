//! Ribbit on Android: the shared frontend inside a NativeActivity.
//!
//! Settings and saves live in the app's private storage. To open a BIOS or
//! cartridge from anywhere (Downloads, SD card) the in-app file browser needs
//! "All files access", which Android only grants from its settings screen;
//! the app opens that screen once if the permission is missing.

#![cfg(target_os = "android")]

use std::path::PathBuf;
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
    let event_loop = match EventLoop::builder().with_android_app(app).build() {
        Ok(l) => l,
        Err(e) => {
            log::error!("event loop: {e}");
            return;
        }
    };
    let opts = ribbit_frontend::StartOptions { data_dir, ..Default::default() };
    if let Err(e) = ribbit_frontend::run_with(event_loop, opts) {
        log::error!("ribbit stopped: {e:#}");
    }
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
