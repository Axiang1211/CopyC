//! 前端可呼叫的指令。

use crate::{
    classify::{Clip, Payload, SensitiveReason},
    hud, monitor, platform,
    settings::Settings,
    state::{AppState, HistoryOut, Stats},
    tray,
};
use serde::Serialize;
use std::{thread, time::Duration};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;

/// 儲存並套用設定，同步系統匣選單與所有視窗。
pub fn commit_settings(app: &AppHandle, next: Settings) -> Settings {
    let next = next.sanitized();
    let state = app.state::<AppState>();
    *state.settings.lock() = next.clone();
    if let Err(e) = next.save(&state.settings_path) {
        eprintln!("ctrl-c: 無法儲存設定：{e}");
    }
    tray::refresh(app);
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
    version: String,
    stats: Stats,
    history: Vec<HistoryOut>,
    recent_apps: Vec<String>,
    autostart: bool,
}

#[tauri::command]
pub fn get_overview(app: AppHandle, state: tauri::State<'_, AppState>) -> Overview {
    Overview {
        platform: platform::PLATFORM,
        version: app.package_info().version.to_string(),
        stats: *state.stats.lock(),
        history: state.history_out(0, 10),
        recent_apps: state.recent_apps.lock().clone(),
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
    }
}

#[tauri::command]
pub fn clear_history(app: AppHandle, state: tauri::State<'_, AppState>) {
    state.history.lock().clear();
    let _ = app.emit("overview:changed", ());
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> bool {
    let launcher = app.autolaunch();
    let _ = if enabled { launcher.enable() } else { launcher.disable() };
    launcher.is_enabled().unwrap_or(false)
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

fn sample(kind: &str) -> Payload {
    match kind {
        "image" => Payload::Image { width: 1920, height: 1080, format: "PNG".into(), thumb: sample_thumb() },
        "files" => Payload::Files {
            first: "專案進度報告_v2.docx".into(),
            count: 4,
            is_dir: false,
            ext: "docx".into(),
            icon: None,
        },
        "file" => Payload::Files { first: "要求.pdf".into(), count: 1, is_dir: false, ext: "pdf".into(), icon: None },
        "color" => Payload::Color { value: "#10B981".into() },
        "url" => Payload::Url {
            scheme: "https://".into(),
            domain: "example.com".into(),
            rest: "/products/ctrl-c?ref=to…".into(),
        },
        "sensitive" => Payload::Sensitive { reason: SensitiveReason::Entropy },
        "private" => Payload::Private,
        _ => Payload::Text {
            preview: "這是一段測試文字內容，讓你確認 Ctrl-C 正常".into(),
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
    let payload = sample(base);
    let payload = if app.state::<AppState>().settings.lock().privacy_mode { Payload::Private } else { payload };
    let clip = Clip {
        payload,
        hash: base.bytes().fold(0xC0FFEEu64, |h, b| h.rotate_left(5) ^ b as u64),
        source: Some("Ctrl-C".into()),
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
