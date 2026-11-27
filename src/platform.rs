//! Bridge to the Android media notification / foreground service (no-op on desktop).

pub const ACTION_TOGGLE: i32 = 1;
pub const ACTION_NEXT: i32 = 2;
pub const ACTION_PREV: i32 = 3;
pub const ACTION_PAUSE: i32 = 4;
pub const ACTION_PLAY: i32 = 5;
// Negative queue entries encode seek positions as -(milliseconds + 1).
pub fn seek_seconds(action: i64) -> Option<f32> {
    (action < 0).then(|| (-(action + 1)) as f32 / 1000.0)
}
/// Pause and release the audio output: another app took over.
pub const ACTION_PARK: i32 = 6;
pub const ACTION_RESCAN: i32 = 10;
pub const ACTION_FOLDER: i32 = 11;
pub const ACTION_IMAGE: i32 = 12;

#[cfg(not(target_os = "android"))]
pub fn now_playing(_title: &str, _artist: &str, _playing: bool, _pos: i64, _duration: i64) {}

#[cfg(not(target_os = "android"))]
pub fn poll_action() -> Option<i64> {
    None
}

#[cfg(not(target_os = "android"))]
pub fn can_write_all() -> bool {
    true
}

#[cfg(not(target_os = "android"))]
pub fn ask_write_all() {}

/// Opens the system folder picker; returns false when there is none.
#[cfg(not(target_os = "android"))]
pub fn pick_folder() -> bool {
    false
}

/// Opens the system image picker; returns false when there is none.
#[cfg(not(target_os = "android"))]
pub fn pick_image() -> bool {
    false
}

#[cfg(not(target_os = "android"))]
pub fn take_picked(_image: bool) -> String {
    String::new()
}

/// Root and app-owned folder of the removable SD card, if one is mounted.
#[cfg(not(target_os = "android"))]
pub fn sd_card() -> Option<(String, String)> {
    None
}

#[cfg(target_os = "android")]
mod android {
    use jni::objects::{GlobalRef, JClass, JObject, JValue};
    use jni::{JNIEnv, JavaVM};
    use std::sync::OnceLock;

    static BRIDGE: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();

    pub fn init_bridge() {
        if BRIDGE.get().is_some() {
            return;
        }
        let ctx = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.expect("Java VM");
        let mut env = vm
            .attach_current_thread_permanently()
            .expect("JNI attachment");
        let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
        let class = env
            .with_local_frame(8, |env| -> jni::errors::Result<GlobalRef> {
                let loader = env
                    .call_method(
                        &activity,
                        "getClassLoader",
                        "()Ljava/lang/ClassLoader;",
                        &[],
                    )?
                    .l()?;
                let name = env.new_string("com.tein.app.TeinBridge")?;
                let class: JClass = env
                    .call_method(
                        &loader,
                        "loadClass",
                        "(Ljava/lang/String;)Ljava/lang/Class;",
                        &[JValue::Object(&name)],
                    )?
                    .l()?
                    .into();
                env.new_global_ref(class)
            })
            .expect("Tein bridge class");
        let _ = BRIDGE.set((vm, class));
    }

    fn with_bridge<T>(f: impl FnOnce(&mut JNIEnv, &JClass) -> jni::errors::Result<T>) -> Option<T> {
        let (vm, class) = BRIDGE.get()?;
        let mut env = vm.attach_current_thread_permanently().ok()?;
        let result = env.with_local_frame(8, |env| f(env, class.as_obj().into()));
        if result.is_err() && env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        result.map_err(|e| log::warn!("JNI bridge error: {e}")).ok()
    }

    pub fn now_playing(title: &str, artist: &str, playing: bool, pos: i64, duration: i64) {
        with_bridge(|env, class| {
            let t = env.new_string(title)?;
            let a = env.new_string(artist)?;
            env.call_static_method(
                class,
                "update",
                "(Ljava/lang/String;Ljava/lang/String;ZJJ)V",
                &[
                    JValue::Object(&t),
                    JValue::Object(&a),
                    JValue::Bool(playing as u8),
                    JValue::Long(pos),
                    JValue::Long(duration),
                ],
            )?;
            env.delete_local_ref(t)?;
            env.delete_local_ref(a)?;
            Ok(())
        });
    }

    pub fn poll_action() -> Option<i64> {
        with_bridge(|env, class| env.call_static_method(class, "poll", "()J", &[])?.j())
            .filter(|a| *a != 0)
    }

    pub fn can_write_all() -> bool {
        with_bridge(|env, class| {
            env.call_static_method(class, "canWriteAll", "()Z", &[])?
                .z()
        })
        .unwrap_or(true)
    }

    fn call_string(name: &str) -> String {
        with_bridge(|env, class| {
            let obj = env
                .call_static_method(class, name, "()Ljava/lang/String;", &[])?
                .l()?;
            let text: String = env.get_string(&obj.into())?.into();
            Ok(text)
        })
        .unwrap_or_default()
    }

    fn call_void(name: &str) -> bool {
        with_bridge(|env, class| {
            env.call_static_method(class, name, "()V", &[])?;
            Ok(())
        })
        .is_some()
    }

    pub fn pick_folder() -> bool {
        call_void("pickFolder")
    }

    pub fn pick_image() -> bool {
        call_void("pickImage")
    }

    pub fn take_picked(image: bool) -> String {
        call_string(if image { "takeImage" } else { "takeFolder" })
    }

    pub fn sd_card() -> Option<(String, String)> {
        let text = call_string("sdCard");
        let (root, app) = text.split_once('\n')?;
        Some((root.to_string(), app.to_string()))
    }

    pub fn ask_write_all() {
        with_bridge(|env, class| {
            env.call_static_method(class, "askWriteAll", "()V", &[])?;
            Ok(())
        });
    }
}

#[cfg(target_os = "android")]
pub use android::{
    ask_write_all, can_write_all, init_bridge, now_playing, pick_folder, pick_image, poll_action,
    sd_card, take_picked,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeks_are_distinct_from_commands_and_keep_milliseconds() {
        assert_eq!(seek_seconds(0), None);
        assert_eq!(seek_seconds(ACTION_RESCAN.into()), None);
        assert_eq!(seek_seconds(-1), Some(0.0));
        assert_eq!(seek_seconds(-65_501), Some(65.5));
    }
}
