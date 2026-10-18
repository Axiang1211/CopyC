//! 剪貼簿管線：系統事件 → 讀取快照 → 智慧識別 → 通知框。

use crate::{
    classify::{self, FileEntry, ImageInfo, Snapshot},
    hud, platform,
    settings::Settings,
    state::AppState,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{io::Cursor, sync::mpsc, thread, time::Duration};
use tauri::{AppHandle, Manager};

/// 等寫入方完成（部分 App 會分多次寫入格式），同時合併同一次複製觸發的多個事件。
const SETTLE: Duration = Duration::from_millis(40);
/// 縮圖輸出 64px（顯示 32px，2× 高 DPI）。
const THUMB_PX: u32 = 64;

pub fn start(app: AppHandle) {
    let (tx, rx) = mpsc::channel::<()>();
    platform::start_clipboard_listener(tx);
    let _ = thread::Builder::new().name("ctrl-c-clipboard".into()).spawn(move || {
        platform::init_worker_thread();
        while rx.recv().is_ok() {
            thread::sleep(SETTLE);
            while rx.try_recv().is_ok() {}
            let settings = app.state::<AppState>().settings.lock().clone();
            let snap = read_snapshot(&settings);
            hud::handle(&app, classify::classify(snap, &settings));
        }
    });
}

fn read_snapshot(settings: &Settings) -> Snapshot {
    let native = platform::read_native();
    let mut snap = Snapshot {
        format_count: native.format_count,
        sensitive_marker: native.sensitive_marker,
        source: native.source,
        ..Default::default()
    };
    if snap.format_count == 0 {
        return snap;
    }

    let detailed = !settings.privacy_mode && !snap.sensitive_marker;
    if !native.files.is_empty() {
        let want_icon = detailed && settings.smart_types.files;
        snap.files = native
            .files
            .into_iter()
            .enumerate()
            .map(|(i, path)| {
                let icon = if i == 0 && want_icon { platform::file_icon_png(&path).map(png_data_url) } else { None };
                FileEntry { path, icon }
            })
            .collect();
        return snap;
    }

    let Ok(mut cb) = arboard::Clipboard::new() else { return snap };
    snap.text = cb.get_text().ok().filter(|t| !t.is_empty());
    if snap.text.is_none() && native.has_image {
        if let Ok(img) = cb.get_image() {
            let thumb = (detailed && settings.smart_types.image)
                .then(|| thumbnail(img.width as u32, img.height as u32, img.bytes.into_owned()))
                .flatten();
            snap.image = Some(ImageInfo {
                width: img.width as u32,
                height: img.height as u32,
                format: native.image_format.unwrap_or_default(),
                thumb,
            });
        }
    }
    snap
}

pub fn png_data_url(png: Vec<u8>) -> String {
    format!("data:image/png;base64,{}", STANDARD.encode(png))
}

/// 等比縮小到短邊 64px，前端再以 object-fit: cover 裁成 32×32 圓角方塊。
pub fn thumbnail(width: u32, height: u32, rgba: Vec<u8>) -> Option<String> {
    if width == 0 || height == 0 {
        return None;
    }
    let img = image::RgbaImage::from_raw(width, height, rgba)?;
    let scale = (THUMB_PX as f64 / width.min(height) as f64).min(1.0);
    let tw = ((width as f64 * scale).round() as u32).clamp(1, THUMB_PX * 4);
    let th = ((height as f64 * scale).round() as u32).clamp(1, THUMB_PX * 4);
    let small = image::imageops::thumbnail(&img, tw, th);
    let mut png = Vec::new();
    small.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png_data_url(png))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_keeps_short_side_at_64() {
        let (w, h) = (1920u32, 1080u32);
        let url = thumbnail(w, h, vec![200; (w * h * 4) as usize]).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        let png = STANDARD.decode(&url["data:image/png;base64,".len()..]).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(img.height(), 64);
        assert_eq!(img.width(), 114);
    }

    #[test]
    fn thumbnail_rejects_bad_buffers() {
        assert!(thumbnail(0, 10, vec![]).is_none());
        assert!(thumbnail(10, 10, vec![0; 3]).is_none());
    }
}
