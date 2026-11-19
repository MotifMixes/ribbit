//! The rest of libandroid the titles import: sensors, the looper and native windows.
//! The accelerometer is there but sends no events yet; the looper has nothing to deliver.

use crate::runtime::{Runtime, Stop};

/// Handles handed out (never dereferenced by the games).
const SENSOR_MANAGER: u32 = 0x0A5E_0000;
const SENSOR: u32 = 0x0A5E_0100;
const EVENT_QUEUE: u32 = 0x0A5E_0200;
const LOOPER: u32 = 0x0A5E_0300;
const WINDOW: u32 = 0x0A5E_0400;
/// ALOOPER_POLL_TIMEOUT
const POLL_TIMEOUT: u32 = -3i32 as u32;

pub fn call(rt: &mut Runtime, name: &str) -> Result<(), Stop> {
    match name {
        "ASensorManager_getInstance" => rt.ret(SENSOR_MANAGER),
        "ASensorManager_getDefaultSensor" => rt.ret(SENSOR),
        "ASensorManager_createEventQueue" => rt.ret(EVENT_QUEUE),
        "ASensorManager_destroyEventQueue"
        | "ASensorEventQueue_enableSensor"
        | "ASensorEventQueue_disableSensor"
        | "ASensorEventQueue_setEventRate" => rt.ret(0),
        // No events: 0 read.
        "ASensorEventQueue_getEvents" => rt.ret(0),
        "ALooper_forThread" | "ALooper_prepare" => rt.ret(LOOPER),
        "ALooper_pollAll" | "ALooper_pollOnce" => rt.ret(POLL_TIMEOUT),
        "ANativeWindow_fromSurface" => rt.ret(WINDOW),
        "ANativeWindow_release" | "ANativeWindow_acquire" => rt.ret(0),
        "ANativeWindow_getWidth" => rt.ret(rt.screen.0),
        "ANativeWindow_getHeight" => rt.ret(rt.screen.1),
        _ => return Err(Stop::Import(name.to_string(), rt.cpu.r[14])),
    }
    Ok(())
}
