//! Linux 等非目標平台的開發用替代實作：以輪詢比對內容取代系統剪貼簿事件。
//! 正式支援的平台為 Windows 與 macOS，這裡只求能在開發環境跑起來。

use super::NativeClip;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::Path,
    sync::mpsc::Sender,
    thread,
    time::Duration,
};

pub fn platform_setup() {}

pub fn init_worker_thread() {}

pub fn start_clipboard_listener(tx: Sender<()>) {
    thread::spawn(move || {
        let Ok(mut cb) = arboard::Clipboard::new() else { return };
        let mut last = signature(&mut cb);
        loop {
            thread::sleep(Duration::from_millis(400));
            let sig = signature(&mut cb);
            if sig != last {
                last = sig;
                if tx.send(()).is_err() {
                    return;
                }
            }
        }
    });
}

fn signature(cb: &mut arboard::Clipboard) -> Option<u64> {
    let mut h = DefaultHasher::new();
    if let Ok(t) = cb.get_text() {
        t.hash(&mut h);
        return Some(h.finish());
    }
    if let Ok(img) = cb.get_image() {
        (img.width, img.height, img.bytes.len()).hash(&mut h);
        return Some(h.finish());
    }
    None
}

pub fn read_native() -> NativeClip {
    let Ok(mut cb) = arboard::Clipboard::new() else { return NativeClip::default() };
    let has_text = cb.get_text().is_ok_and(|t| !t.is_empty());
    let has_image = !has_text && cb.get_image().is_ok();
    NativeClip {
        format_count: usize::from(has_text) + usize::from(has_image),
        has_image,
        ..Default::default()
    }
}

pub fn is_fullscreen() -> bool {
    false
}

pub fn file_icon_png(_path: &Path) -> Option<Vec<u8>> {
    None
}

pub fn prepare_hud(_window: &tauri::WebviewWindow) {}

pub fn show_hud(window: &tauri::WebviewWindow) {
    let _ = window.show();
    // GTK 視窗要實際顯示後才有 GdkWindow，滑鼠穿透只能在這裡設
    let _ = window.set_ignore_cursor_events(true);
}

pub fn hide_hud(window: &tauri::WebviewWindow) {
    let _ = window.hide();
}
