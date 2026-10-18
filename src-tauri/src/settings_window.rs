//! 偏好設定視窗：按需建立、關閉即銷毀，平時不佔用 WebView 記憶體。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "settings";

pub fn open(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(LABEL) {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }

    let builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("settings.html".into()))
        .title("Ctrl-C 偏好設定")
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
