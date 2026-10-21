//! 系統匣（Windows 右下角）/ 選單列（macOS 頂部）圖示與簡易選單。

use crate::{
    commands::commit_settings,
    i18n,
    settings::{Language, Position, Settings},
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

/// 位置與選單 id；顯示名稱依序取自 i18n::Strings::positions。
pub const POSITIONS: [(Position, &str); 6] = [
    (Position::Top, "top"),
    (Position::Cursor, "cursor"),
    (Position::TopLeft, "top-left"),
    (Position::TopRight, "top-right"),
    (Position::BottomLeft, "bottom-left"),
    (Position::BottomRight, "bottom-right"),
];
/// 語言選項：選單 id、設定值、固定顯示名稱（語言名稱本身不翻譯）。
const LANGUAGES: [(&str, Language, Option<&str>); 8] = [
    ("auto", Language::Auto, None),
    ("zh-hant", Language::ZhHant, Some("繁體中文")),
    ("en", Language::En, Some("English")),
    ("zh-hans", Language::ZhHans, Some("简体中文")),
    ("ja", Language::Ja, Some("日本語")),
    ("ko", Language::Ko, Some("한국어")),
    ("fr", Language::Fr, Some("Français")),
    ("ms", Language::Ms, Some("Bahasa Melayu")),
];
fn as_refs(items: &[CheckMenuItem<Wry>]) -> Vec<&dyn IsMenuItem<Wry>> {
    items.iter().map(|i| i as &dyn IsMenuItem<Wry>).collect()
}

const DURATIONS: [u32; 5] = [800, 1200, 1400, 2000, 3000];
const OPACITIES: [u32; 4] = [100, 90, 80, 70];

const BRAND: &str = "CopyC";

fn build_menu(app: &AppHandle, s: &Settings) -> tauri::Result<Menu<Wry>> {
    let t = i18n::strings(i18n::resolve(s.language));
    let header = MenuItem::with_id(app, "header", format!("{BRAND} {}", t.watching), false, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", t.pause, true, s.paused, None::<&str>)?;

    let positions = POSITIONS
        .iter()
        .zip(t.positions)
        .map(|((p, key), label)| CheckMenuItem::with_id(app, format!("pos:{key}"), label, true, s.position == *p, None::<&str>))
        .collect::<tauri::Result<Vec<_>>>()?;
    let durations = DURATIONS
        .iter()
        .map(|ms| {
            let number = format!("{:.1}", *ms as f64 / 1000.0);
            let number = if i18n::resolve(s.language) == i18n::Lang::Fr {
                number.replace('.', ",")
            } else { number };
            let label = format!("{number} {}", t.seconds);
            CheckMenuItem::with_id(app, format!("dur:{ms}"), label, true, s.duration_ms == *ms, None::<&str>)
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let opacities = OPACITIES
        .iter()
        .map(|pct| {
            let checked = ((s.opacity * 100.0).round() as u32) == *pct;
            let label = if i18n::resolve(s.language) == i18n::Lang::Fr {
                format!("{pct}\u{a0}%")
            } else { format!("{pct}%") };
            CheckMenuItem::with_id(app, format!("op:{pct}"), label, true, checked, None::<&str>)
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let languages = LANGUAGES
        .iter()
        .map(|(key, lang, name)| {
            let label = name.unwrap_or(t.lang_auto);
            CheckMenuItem::with_id(app, format!("lang:{key}"), label, true, s.language == *lang, None::<&str>)
        })
        .collect::<tauri::Result<Vec<_>>>()?;

    let position_menu = Submenu::with_items(app, t.position, true, &as_refs(&positions))?;
    let duration_menu = Submenu::with_items(app, t.duration, true, &as_refs(&durations))?;
    let opacity_menu = Submenu::with_items(app, t.opacity, true, &as_refs(&opacities))?;
    let language_menu = Submenu::with_items(app, t.language, true, &as_refs(&languages))?;

    let sound = CheckMenuItem::with_id(app, "sound", t.sound, true, s.sound, None::<&str>)?;
    let privacy = CheckMenuItem::with_id(app, "privacy", t.privacy, true, s.privacy_mode, None::<&str>)?;
    let test = MenuItem::with_id(app, "test", t.try_it, true, None::<&str>)?;
    let prefs = MenuItem::with_id(app, "settings", t.settings, true, Some("CmdOrCtrl+,"))?;
    let quit = MenuItem::with_id(app, "quit", format!("{} {BRAND}", t.quit), true, Some("CmdOrCtrl+Q"))?;
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
            &language_menu,
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
        .tooltip(tooltip(&settings))
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

fn tooltip(s: &Settings) -> String {
    format!("{BRAND} — {}", i18n::strings(i18n::resolve(s.language)).tooltip)
}

/// 設定變更後重建選單，讓勾選狀態、語言與設定視窗保持一致。
pub fn refresh(app: &AppHandle) {
    let settings = app.state::<AppState>().settings.lock().clone();
    if let (Some(tray), Ok(menu)) = (app.tray_by_id(TRAY_ID), build_menu(app, &settings)) {
        let _ = tray.set_menu(Some(menu));
        let _ = tray.set_tooltip(Some(tooltip(&settings)));
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
                match POSITIONS.iter().find(|(_, k)| *k == key) {
                    Some((p, _)) => s.position = *p,
                    None => return,
                }
            } else if let Some(key) = id.strip_prefix("lang:") {
                match LANGUAGES.iter().find(|(k, _, _)| *k == key) {
                    Some((_, lang, _)) => s.language = *lang,
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
