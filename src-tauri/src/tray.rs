//! 系統匣（Windows 右下角）/ 選單列（macOS 頂部）圖示與簡易選單。

use crate::{
    commands::commit_settings,
    settings::{Position, Settings},
    settings_window,
    state::AppState,
};
use tauri::{
    image::Image,
    menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    AppHandle, Manager, Wry,
};

const TRAY_ID: &str = "copyc";

pub const POSITIONS: [(Position, &str, &str); 6] = [
    (Position::Top, "top", "螢幕正上方膠囊"),
    (Position::Cursor, "cursor", "游標旁微氣泡"),
    (Position::TopLeft, "top-left", "左上角"),
    (Position::TopRight, "top-right", "右上角"),
    (Position::BottomLeft, "bottom-left", "左下角"),
    (Position::BottomRight, "bottom-right", "右下角"),
];
fn as_refs(items: &[CheckMenuItem<Wry>]) -> Vec<&dyn IsMenuItem<Wry>> {
    items.iter().map(|i| i as &dyn IsMenuItem<Wry>).collect()
}

const DURATIONS: [u32; 5] = [800, 1200, 1400, 2000, 3000];
const OPACITIES: [u32; 4] = [100, 90, 80, 70];

const BRAND: &str = "CopyC";

fn build_menu(app: &AppHandle, s: &Settings) -> tauri::Result<Menu<Wry>> {
    let header = MenuItem::with_id(app, "header", format!("{} 正在守護你的剪貼簿", BRAND), false, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", "暫停提示", true, s.paused, None::<&str>)?;

    let positions = POSITIONS
        .iter()
        .map(|(p, key, label)| CheckMenuItem::with_id(app, format!("pos:{key}"), *label, true, s.position == *p, None::<&str>))
        .collect::<tauri::Result<Vec<_>>>()?;
    let durations = DURATIONS
        .iter()
        .map(|ms| {
            let label = format!("{:.1} 秒", *ms as f64 / 1000.0);
            CheckMenuItem::with_id(app, format!("dur:{ms}"), label, true, s.duration_ms == *ms, None::<&str>)
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let opacities = OPACITIES
        .iter()
        .map(|pct| {
            let checked = ((s.opacity * 100.0).round() as u32) == *pct;
            CheckMenuItem::with_id(app, format!("op:{pct}"), format!("{pct}%"), true, checked, None::<&str>)
        })
        .collect::<tauri::Result<Vec<_>>>()?;

    let position_menu = Submenu::with_items(app, "顯示位置", true, &as_refs(&positions))?;
    let duration_menu = Submenu::with_items(app, "停留秒數", true, &as_refs(&durations))?;
    let opacity_menu = Submenu::with_items(app, "透明度", true, &as_refs(&opacities))?;

    let sound = CheckMenuItem::with_id(app, "sound", "提示音效", true, s.sound, None::<&str>)?;
    let privacy = CheckMenuItem::with_id(app, "privacy", "隱私打碼模式（分享螢幕用）", true, s.privacy_mode, None::<&str>)?;
    let test = MenuItem::with_id(app, "test", "試一下提示", true, None::<&str>)?;
    let prefs = MenuItem::with_id(app, "settings", "偏好設定…", true, Some("CmdOrCtrl+,"))?;
    let quit = MenuItem::with_id(app, "quit", format!("結束 {}", BRAND), true, Some("CmdOrCtrl+Q"))?;
    let sep = || PredefinedMenuItem::separator(app);

    Menu::with_items(
        app,
        &[
            &header,
            &sep()?,
            &pause,
            &position_menu,
            &duration_menu,
            &opacity_menu,
            &sound,
            &privacy,
            &sep()?,
            &test,
            &prefs,
            &sep()?,
            &quit,
        ],
    )
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let settings = app.state::<AppState>().settings.lock().clone();
    let menu = build_menu(app, &settings)?;
    let mac = cfg!(target_os = "macos");
    // macOS 選單列使用單色 template 圖示，跟隨系統深淺色
    let icon = if mac {
        Image::from_bytes(include_bytes!("../icons/tray-template.png"))?
    } else {
        Image::from_bytes(include_bytes!("../icons/tray.png"))?
    };

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(mac)
        .tooltip(format!("{} — 複製即時回饋", BRAND))
        .menu(&menu)
        .show_menu_on_left_click(mac)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|_tray, _event| {
            #[cfg(not(target_os = "macos"))]
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = _event
            {
                settings_window::open(_tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// 設定變更後重建選單，讓勾選狀態與設定視窗保持一致。
pub fn refresh(app: &AppHandle) {
    let settings = app.state::<AppState>().settings.lock().clone();
    if let (Some(tray), Ok(menu)) = (app.tray_by_id(TRAY_ID), build_menu(app, &settings)) {
        let _ = tray.set_menu(Some(menu));
    }
}

fn on_menu(app: &AppHandle, id: &str) {
    let mut s = app.state::<AppState>().settings.lock().clone();
    match id {
        "pause" => s.paused = !s.paused,
        "sound" => s.sound = !s.sound,
        "privacy" => s.privacy_mode = !s.privacy_mode,
        "test" => return crate::commands::run_preview(app, "text"),
        "settings" => return settings_window::open(app),
        "quit" => return app.exit(0),
        _ => {
            if let Some(key) = id.strip_prefix("pos:") {
                match POSITIONS.iter().find(|(_, k, _)| *k == key) {
                    Some((p, _, _)) => s.position = *p,
                    None => return,
                }
            } else if let Some(ms) = id.strip_prefix("dur:").and_then(|v| v.parse().ok()) {
                s.duration_ms = ms;
            } else if let Some(pct) = id.strip_prefix("op:").and_then(|v| v.parse::<u32>().ok()) {
                s.opacity = pct as f32 / 100.0;
            } else {
                return;
            }
        }
    }
    commit_settings(app, s);
}
