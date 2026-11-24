//! Bridge to the Android media notification / foreground service (no-op on desktop).

pub const ACTION_TOGGLE: i32 = 1;
pub const ACTION_NEXT: i32 = 2;
pub const ACTION_PREV: i32 = 3;
pub const ACTION_PAUSE: i32 = 4;
pub const ACTION_PLAY: i32 = 5;
pub const ACTION_RESCAN: i32 = 10;

#[cfg(not(target_os = "android"))]
pub fn now_playing(_title: &str, _artist: &str, _playing: bool) {}

#[cfg(not(target_os = "android"))]
pub fn poll_action() -> Option<i32> {
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
    use jni::objects::{JClass, JObject, JValue};
    use jni::{JNIEnv, JavaVM};

    fn with_bridge<T>(f: impl FnOnce(&mut JNIEnv, &JClass) -> jni::errors::Result<T>) -> Option<T> {
        let ctx = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
        let mut env = vm.attach_current_thread_permanently().ok()?;
        let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
        let result = (|| {
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
            let out = f(&mut env, &class);
            env.delete_local_ref(class)?;
            env.delete_local_ref(name)?;
            env.delete_local_ref(loader)?;
            out
        })();
        std::mem::forget(activity);
        if result.is_err() && env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        result.map_err(|e| log::warn!("JNI bridge error: {e}")).ok()
    }

    pub fn now_playing(title: &str, artist: &str, playing: bool) {
        with_bridge(|env, class| {
            let t = env.new_string(title)?;
            let a = env.new_string(artist)?;
            env.call_static_method(
                class,
                "update",
                "(Ljava/lang/String;Ljava/lang/String;Z)V",
                &[
                    JValue::Object(&t),
                    JValue::Object(&a),
                    JValue::Bool(playing as u8),
                ],
            )?;
            env.delete_local_ref(t)?;
            env.delete_local_ref(a)?;
            Ok(())
        });
    }

    pub fn poll_action() -> Option<i32> {
        with_bridge(|env, class| env.call_static_method(class, "poll", "()I", &[])?.i())
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
pub use android::{ask_write_all, can_write_all, now_playing, poll_action};
