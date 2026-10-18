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

/// 用預設瀏覽器開啟網址。
pub fn open_url(url: &str) {
    #[cfg(windows)]
    shell_open(url);
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
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
