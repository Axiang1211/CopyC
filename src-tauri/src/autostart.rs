//! 開機自動啟動。官網版用 tauri-plugin-autostart（Windows 登錄檔 Run / macOS LaunchAgent）；
//! Microsoft Store（MSIX）版的登錄檔寫入會被虛擬化，改用封裝清單裡的 StartupTask。
//!
//! 兩者都可能呼叫阻塞的系統 API（WinRT 非同步作業），只能在背景執行緒呼叫。

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Autostart {
    pub enabled: bool,
    /// 系統層級鎖定（使用者在 Windows「設定 → 應用程式 → 啟動」關閉，或群組原則），App 無法自行切換
    pub blocked: bool,
}

pub fn state(app: &AppHandle) -> Autostart {
    #[cfg(windows)]
    if crate::platform::is_packaged() {
        return crate::platform::startup_task_state();
    }
    Autostart { enabled: app.autolaunch().is_enabled().unwrap_or(false), blocked: false }
}

pub fn set(app: &AppHandle, enabled: bool) -> Autostart {
    #[cfg(windows)]
    if crate::platform::is_packaged() {
        return crate::platform::set_startup_task(enabled);
    }
    let launcher = app.autolaunch();
    let _ = if enabled { launcher.enable() } else { launcher.disable() };
    state(app)
}
