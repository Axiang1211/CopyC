//! 呼叫 Android 端的 CopyCBridge（Kotlin）：寫入剪貼簿、顯示通知、開關背景服務、區域網路設定。
//! App 在背景（只剩背景服務）時也能用，因為用的是 Application context，不是畫面。
//! 桌面上開發時這些都是空操作。

#[cfg(target_os = "android")]
mod imp {
    use jni::{
        objects::{JClass, JObject, JValue},
        JNIEnv, JavaVM,
    };

    const BRIDGE: &str = "top.dreamsworld.copyc.CopyCBridge";

    /// 取得 CopyCBridge 類別後執行 `f`。必須用 App 的 ClassLoader 載入：
    /// Rust 自己開的執行緒用 FindClass 只找得到系統類別。
    fn with_bridge<T>(f: impl FnOnce(&mut JNIEnv, &JClass, &JObject) -> jni::errors::Result<T>) -> Option<T> {
        let ctx = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
        let mut env = vm.attach_current_thread().ok()?;
        // tao 放進 ndk_context 的是 Application context（全域參照），App 存活期間都有效
        let context = unsafe { JObject::from_raw(ctx.context().cast()) };
        let result = (|| {
            let loader = env.call_method(&context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
            let name = env.new_string(BRIDGE)?;
            let class = env
                .call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&name)])?
                .l()?;
            let class = JClass::from(class);
            f(&mut env, &class, &context)
        })();
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        result.ok()
    }

    pub fn set_clipboard(text: &str) -> bool {
        with_bridge(|env, class, ctx| {
            let s = env.new_string(text)?;
            env.call_static_method(
                class,
                "setClipboard",
                "(Landroid/content/Context;Ljava/lang/String;)V",
                &[JValue::Object(ctx), JValue::Object(&s)],
            )?;
            Ok(())
        })
        .is_some()
    }

    pub fn notify_received(device: &str, preview: &str) {
        with_bridge(|env, class, ctx| {
            let d = env.new_string(device)?;
            let p = env.new_string(preview)?;
            env.call_static_method(
                class,
                "notifyReceived",
                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V",
                &[JValue::Object(ctx), JValue::Object(&d), JValue::Object(&p)],
            )?;
            Ok(())
        });
    }

    pub fn set_background(on: bool) {
        with_bridge(|env, class, ctx| {
            env.call_static_method(
                class,
                "setBackground",
                "(Landroid/content/Context;Z)V",
                &[JValue::Object(ctx), JValue::Bool(u8::from(on))],
            )?;
            Ok(())
        });
    }

    pub fn set_multicast(on: bool) {
        with_bridge(|env, class, ctx| {
            env.call_static_method(
                class,
                "setMulticast",
                "(Landroid/content/Context;Z)V",
                &[JValue::Object(ctx), JValue::Bool(u8::from(on))],
            )?;
            Ok(())
        });
    }

    /// 把收到的檔案存到「下載/CopyC」（圖片存到「圖片/CopyC」），回傳 content:// URI。
    pub fn save_received(path: &std::path::Path, name: &str) -> Option<String> {
        with_bridge(|env, class, ctx| {
            let p = env.new_string(path.to_string_lossy())?;
            let n = env.new_string(name)?;
            let obj = env
                .call_static_method(
                    class,
                    "saveReceived",
                    "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                    &[JValue::Object(ctx), JValue::Object(&p), JValue::Object(&n)],
                )?
                .l()?;
            if obj.is_null() {
                return Ok(None);
            }
            let jstr = jni::objects::JString::from(obj);
            Ok(Some(String::from(env.get_string(&jstr)?)))
        })
        .flatten()
    }

    /// 收到檔案的通知：點一下打開。
    pub fn notify_file(device: &str, name: &str, uri: &str) {
        with_bridge(|env, class, ctx| {
            let d = env.new_string(device)?;
            let n = env.new_string(name)?;
            let u = env.new_string(uri)?;
            env.call_static_method(
                class,
                "notifyFile",
                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V",
                &[JValue::Object(ctx), JValue::Object(&d), JValue::Object(&n), JValue::Object(&u)],
            )?;
            Ok(())
        });
    }

    /// 有檔案經由 Google Drive 傳來：通知一下，點一下打開 App 下載。
    pub fn notify_incoming(device: &str, name: &str) {
        with_bridge(|env, class, ctx| {
            let d = env.new_string(device)?;
            let n = env.new_string(name)?;
            env.call_static_method(
                class,
                "notifyIncoming",
                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V",
                &[JValue::Object(ctx), JValue::Object(&d), JValue::Object(&n)],
            )?;
            Ok(())
        });
    }

    pub fn device_name() -> Option<String> {
        with_bridge(|env, class, _| {
            let obj = env.call_static_method(class, "deviceName", "()Ljava/lang/String;", &[])?.l()?;
            let jstr = jni::objects::JString::from(obj);
            let name = String::from(env.get_string(&jstr)?);
            Ok(name)
        })
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    pub fn set_clipboard(_text: &str) -> bool {
        false
    }
    pub fn notify_received(_device: &str, _preview: &str) {}
    pub fn set_background(_on: bool) {}
    pub fn set_multicast(_on: bool) {}
    pub fn device_name() -> Option<String> {
        None
    }
    /// 電腦上開發時：直接複製到暫存資料夾。
    pub fn save_received(path: &std::path::Path, name: &str) -> Option<String> {
        let dest = std::env::temp_dir().join(name);
        std::fs::copy(path, &dest).ok()?;
        Some(dest.to_string_lossy().into_owned())
    }
    pub fn notify_file(_device: &str, _name: &str, _uri: &str) {}
    pub fn notify_incoming(_device: &str, _name: &str) {}
}

pub use imp::*;
