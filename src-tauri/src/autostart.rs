//! 開機自動啟動。官網版用 tauri-plugin-autostart（Windows 登錄檔 Run / macOS LaunchAgent）；
//! Microsoft Store（MSIX）版的登錄檔寫入會被虛擬化，改用封裝清單裡的 StartupTask。
//!
//! 兩者都可能呼叫阻塞的系統 API（WinRT 非同步作業），只能在背景執行緒呼叫。
//!
//! macOS 的 LaunchAgent 記的是「登記當下」App 的位置。沒有 Apple 簽名的 App 直接從 DMG 或「下載」打開時，
//! 系統會把它搬到隨機的暫存位置執行（App Translocation），記到那裡的話重開機就找不到。
//! 所以：在暫存位置時先不登記（記下「之後要開」），等從「應用程式」打開時再登記；每次啟動也重新登記一次，
//! 搬過位置後一樣有效。

use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Autostart {
    pub enabled: bool,
    /// 系統層級鎖定（使用者在 Windows「設定 → 應用程式 → 啟動」關閉，或群組原則），App 無法自行切換
    pub blocked: bool,
    /// macOS：App 正在 DMG 或暫存位置執行，要先拖進「應用程式」才能設定
    pub misplaced: bool,
}

/// macOS：App 現在跑的位置是暫時的（直接從 DMG 開，或被系統從「下載」搬到隨機位置執行）。
pub fn misplaced() -> bool {
    #[cfg(target_os = "macos")]
    {
        let Ok(exe) = std::env::current_exe() else { return false };
        let path = exe.to_string_lossy();
        path.contains("/AppTranslocation/") || path.starts_with("/Volumes/")
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// 在暫存位置時想開啟自動啟動：先記下來，等從正確位置打開時再登記。
fn pending_file(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_config_dir().ok()?.join("autostart-pending"))
}

/// 每次啟動時呼叫（背景執行緒）：macOS 用現在的位置重新登記，補上之前沒能登記的。
pub fn refresh(app: &AppHandle) {
    if !cfg!(target_os = "macos") || misplaced() {
        return;
    }
    let launcher = app.autolaunch();
    let pending = pending_file(app).filter(|p| p.exists());
    if pending.is_some() || launcher.is_enabled().unwrap_or(false) {
        // 重寫 LaunchAgent，路徑換成現在這份 App
        let _ = launcher.enable();
    }
    if let Some(p) = pending {
        let _ = std::fs::remove_file(p);
    }
}

pub fn state(app: &AppHandle) -> Autostart {
    #[cfg(windows)]
    if crate::platform::is_packaged() {
        return crate::platform::startup_task_state();
    }
    Autostart { enabled: app.autolaunch().is_enabled().unwrap_or(false), blocked: false, misplaced: misplaced() }
}

pub fn set(app: &AppHandle, enabled: bool) -> Autostart {
    #[cfg(windows)]
    if crate::platform::is_packaged() {
        return crate::platform::set_startup_task(enabled);
    }
    if misplaced() {
        // 先不登記暫時的位置；記下使用者想要的，從「應用程式」打開時 refresh 會補上
        if let Some(p) = pending_file(app) {
            let _ = if enabled { std::fs::write(&p, b"1") } else { std::fs::remove_file(&p) };
        }
        return state(app);
    }
    let launcher = app.autolaunch();
    let _ = if enabled { launcher.enable() } else { launcher.disable() };
    state(app)
}
