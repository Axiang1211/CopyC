//! 平台相關實作：剪貼簿事件監聽、來源應用、全螢幕偵測、系統檔案圖示，
//! 以及「不搶焦點」的通知框顯示 / 隱藏。

use serde::Serialize;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(not(any(windows, target_os = "macos")))]
mod fallback;
#[cfg(not(any(windows, target_os = "macos")))]
pub use fallback::*;

/// 是否以 MSIX（Microsoft Store）封裝執行。只有 Windows 有封裝版本。
#[cfg(not(windows))]
pub fn is_packaged() -> bool {
    false
}

/// Microsoft Store 的更新只有 Windows 封裝版會用到。
#[cfg(not(windows))]
pub const STORE_PAGE: &str = "";
#[cfg(not(windows))]
pub fn store_update() -> Result<Option<String>, String> {
    Ok(None)
}
#[cfg(not(windows))]
pub fn store_install(_owner: isize) -> Result<bool, String> {
    Ok(false)
}
#[cfg(not(windows))]
pub fn restart_after_update() {}

/// 把通知框擺到 (x, y)、大小 w×h（實體像素）。Windows 另有實作，見 windows.rs。
#[cfg(not(windows))]
pub fn place_hud(window: &tauri::WebviewWindow, x: i32, y: i32, w: u32, h: u32) {
    let _ = window.set_size(tauri::PhysicalSize::new(w, h));
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}

/// 通知框要不要接收滑鼠（Windows 另有實作，見 windows.rs）。
#[cfg(not(windows))]
pub fn set_hud_clickable(window: &tauri::WebviewWindow, clickable: bool) {
    let _ = window.set_ignore_cursor_events(!clickable);
}

/// 用預設瀏覽器開啟網址。
pub fn open_url(url: &str) {
    #[cfg(windows)]
    shell_open(url);
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// 螢幕頂端的鏡頭區（MacBook 的靈動島或瀏海），實體像素、相對於螢幕左上角。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Housing {
    pub left: f64,
    pub width: f64,
    pub height: f64,
    /// 靈動島（PROLED XDR 螢幕的 MacBook Pro）；false 是瀏海
    pub island: bool,
}

/// 複製來源應用程式。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AppInfo {
    /// 顯示名稱（macOS localizedName / Windows 執行檔名）
    pub name: String,
    /// Bundle ID（macOS）或執行檔完整路徑（Windows）
    pub id: String,
}

/// 由原生 API 讀取、arboard 拿不到的剪貼簿資訊。
#[derive(Debug, Default)]
pub struct NativeClip {
    pub format_count: usize,
    pub sensitive_marker: bool,
    pub files: Vec<PathBuf>,
    pub has_image: bool,
    /// 影像原始格式標籤，例如 PNG / TIFF / BMP。
    pub image_format: Option<String>,
    pub source: Option<AppInfo>,
}

pub const PLATFORM: &str = if cfg!(target_os = "macos") {
    "macos"
} else if cfg!(windows) {
    "windows"
} else {
    "linux"
};
