//! 偏好設定視窗：按需建立、關閉即銷毀，平時不佔用 WebView 記憶體。

use crate::{i18n, state::AppState};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "settings";

pub fn open(app: &AppHandle) {
    open_page(app, None);
}

/// 打開偏好設定並捲到某一段（例如系統匣的「傳送檔案…」→ `transfer`）。
pub fn open_at(app: &AppHandle, section: &str) {
    open_page(app, Some(section));
}

fn open_page(app: &AppHandle, section: Option<&str>) {
    if let Some(win) = app.get_webview_window(LABEL) {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        if let Some(section) = section {
            let _ = win.emit("settings:goto", section);
        }
        return;
    }

    let url = match section {
        Some(section) => format!("settings.html#{section}"),
        None => "settings.html".into(),
    };
    let builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(url.into()))
        .title(title(app))
        .inner_size(940.0, 660.0)
        .min_inner_size(820.0, 580.0)
        .center()
        .focused(true);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(30.0, 34.0));

    if let Ok(win) = builder.build() {
        let _ = win.set_focus();
    }
}

fn title(app: &AppHandle) -> &'static str {
    let lang = i18n::resolve(app.state::<AppState>().settings.lock().language);
    i18n::strings(lang).settings_title
}

/// 切換語言後同步視窗標題。
pub fn retitle(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(LABEL) {
        let _ = win.set_title(title(app));
    }
}
