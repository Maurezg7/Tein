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
pub const ACTION_RESCAN: i32 = 10;

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

    pub fn ask_write_all() {
        with_bridge(|env, class| {
            env.call_static_method(class, "askWriteAll", "()V", &[])?;
            Ok(())
        });
    }
}

#[cfg(target_os = "android")]
pub use android::{ask_write_all, can_write_all, init_bridge, now_playing, poll_action};

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
