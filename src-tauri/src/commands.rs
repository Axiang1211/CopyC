//! 前端可呼叫的指令。

use crate::{
    autostart::{self, Autostart},
    classify::{Clip, Payload, SensitiveReason},
    hud, i18n, monitor, platform,
    settings::Settings,
    state::{AppState, HistoryOut, Stats},
    tray,
};
use serde::Serialize;
use std::{thread, time::Duration};
use tauri::{AppHandle, Emitter, Manager};

/// 儲存並套用設定，同步系統匣選單與所有視窗。
pub fn commit_settings(app: &AppHandle, next: Settings) -> Settings {
    let next = next.sanitized();
    let state = app.state::<AppState>();
    *state.settings.lock() = next.clone();
    if let Err(e) = next.save(&state.settings_path) {
        eprintln!("copyc: 無法儲存設定：{e}");
    }
    tray::refresh(app);
    crate::settings_window::retitle(app);
    let _ = app.emit("settings:changed", &next);
    next
}

#[tauri::command]
pub fn get_settings(state: tauri::State<'_, AppState>) -> Settings {
    state.settings.lock().clone()
}

#[tauri::command]
pub fn update_settings(app: AppHandle, settings: Settings) -> Settings {
    commit_settings(&app, settings)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    platform: &'static str,
    /// 實際使用的介面語言（「跟隨系統」已解析）
    lang: i18n::Lang,
    version: String,
    stats: Stats,
    history: Vec<HistoryOut>,
    recent_apps: Vec<String>,
    /// Microsoft Store（MSIX）版：由商店負責更新
    packaged: bool,
    /// 有新版本時的版本號（不含 v）
    update: Option<String>,
}

#[tauri::command]
pub fn get_overview(app: AppHandle, state: tauri::State<'_, AppState>) -> Overview {
    // 先取出語言並釋放鎖：結構字面值裡的暫時鎖會一直持有到整個運算式結束，
    // 而 history_out 也要拿 settings 鎖，同一執行緒重複上鎖就會卡死。
    let language = state.settings.lock().language;
    Overview {
        platform: platform::PLATFORM,
        lang: i18n::resolve(language),
        version: app.package_info().version.to_string(),
        stats: *state.stats.lock(),
        history: state.history_out(0, 10),
        recent_apps: state.recent_apps.lock().clone(),
        packaged: platform::is_packaged(),
        update: state.update.lock().clone(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    enabled: bool,
    current: String,
}

/// 通知框頁面啟動時呼叫，決定要不要向 GitHub 查詢新版本。
#[tauri::command]
pub fn update_check(app: AppHandle, state: tauri::State<'_, AppState>) -> UpdateCheck {
    let enabled = state.settings.lock().check_updates && !platform::is_packaged();
    UpdateCheck { enabled, current: app.package_info().version.to_string() }
}

/// 發現新版本：記下來並打開偏好設定，由頁面上的提示卡引導使用者前往下載。
#[tauri::command]
pub fn update_found(app: AppHandle, state: tauri::State<'_, AppState>, version: String) {
    let valid = !version.is_empty() && version.len() <= 20 && version.chars().all(|c| c.is_ascii_digit() || c == '.');
    if !valid {
        return;
    }
    {
        let mut update = state.update.lock();
        if update.as_deref() == Some(version.as_str()) {
            return;
        }
        *update = Some(version);
    }
    let _ = app.emit("overview:changed", ());
    crate::settings_window::open(&app);
}

/// 用瀏覽器打開官網下載區（依介面語言選中文或英文頁）。
#[tauri::command]
pub fn open_download(state: tauri::State<'_, AppState>) {
    let lang = i18n::resolve(state.settings.lock().language);
    platform::open_url(match lang {
        i18n::Lang::En => "https://www.copyc.app.dreamsworld.top/en/#download",
        _ => "https://www.copyc.app.dreamsworld.top/#download",
    });
}

#[tauri::command]
pub fn clear_history(app: AppHandle, state: tauri::State<'_, AppState>) {
    state.history.lock().clear();
    let _ = app.emit("overview:changed", ());
}

// 開機啟動會呼叫阻塞的系統 API（MSIX 版是 WinRT 非同步作業），用 async 指令避開主執行緒
#[tauri::command]
pub async fn get_autostart(app: AppHandle) -> Autostart {
    autostart::state(&app)
}

#[tauri::command]
pub async fn set_autostart(app: AppHandle, enabled: bool) -> Autostart {
    autostart::set(&app, enabled)
}

#[tauri::command]
pub fn preview_hud(app: AppHandle, kind: String) {
    run_preview(&app, &kind);
}

#[tauri::command]
pub fn hud_finished(app: AppHandle, seq: u64) {
    hud::finished(&app, seq);
}

#[tauri::command]
pub fn hud_rect(app: AppHandle, x: f64, y: f64, w: f64, h: f64) {
    hud::set_rect(&app, [x, y, w, h]);
}

#[tauri::command]
pub fn open_settings(app: AppHandle) {
    crate::settings_window::open(&app);
}

fn sample_thumb() -> Option<String> {
    let (w, h) = (96u32, 54u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let t = x as f32 / w as f32;
            let v = y as f32 / h as f32;
            rgba.extend_from_slice(&[
                (92.0 + 120.0 * t) as u8,
                (156.0 - 40.0 * v) as u8,
                (230.0 - 90.0 * t) as u8,
                255,
            ]);
        }
    }
    monitor::thumbnail(w, h, rgba)
}

fn sample(kind: &str, lang: i18n::Lang) -> Payload {
    let t = i18n::strings(lang);
    match kind {
        "image" => Payload::Image { width: 1920, height: 1080, format: "PNG".into(), thumb: sample_thumb() },
        "files" => Payload::Files {
            first: t.sample_files.into(),
            count: 4,
            is_dir: false,
            ext: "docx".into(),
            icon: None,
        },
        "file" => Payload::Files { first: t.sample_file.into(), count: 1, is_dir: false, ext: "pdf".into(), icon: None },
        "color" => Payload::Color { value: "#10B981".into() },
        "url" => Payload::Url {
            scheme: "https://".into(),
            domain: "example.com".into(),
            rest: "/products/copyc?ref=to…".into(),
        },
        "sensitive" => Payload::Sensitive { reason: SensitiveReason::Entropy },
        "private" => Payload::Private,
        _ => Payload::Text {
            preview: t.sample_text.into(),
            truncated: true,
            chars: 142,
            lines: 3,
        },
    }
}

/// 「試一下」：repeat 會連送三次同一內容來展示連按彩蛋，dot 展示全螢幕極簡微點。
pub fn run_preview(app: &AppHandle, kind: &str) {
    let base = match kind {
        "repeat" | "dot" => "text",
        k => k,
    };
    let (privacy, lang) = {
        let s = app.state::<AppState>().settings.lock().clone();
        (s.privacy_mode, i18n::resolve(s.language))
    };
    let payload = if privacy { Payload::Private } else { sample(base, lang) };
    let clip = Clip {
        payload,
        hash: base.bytes().fold(0xC0FFEEu64, |h, b| h.rotate_left(5) ^ b as u64),
        source: Some("CopyC".into()),
    };
    match kind {
        "repeat" => {
            let app = app.clone();
            thread::spawn(move || {
                for _ in 0..3 {
                    hud::preview(&app, &clip, false);
                    thread::sleep(Duration::from_millis(260));
                }
            });
        }
        "dot" => hud::preview(app, &clip, true),
        _ => hud::preview(app, &clip, false),
    }
}
