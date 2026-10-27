//! 前端可呼叫的指令。

use crate::{
    autostart::{self, Autostart},
    classify::{Category, Clip, Payload, SensitiveReason},
    cloud::{self, Cloud, CloudError},
    history::{Content, Page},
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
    let prev = std::mem::replace(&mut *state.settings.lock(), next.clone());
    if let Some(store) = state.store.get() {
        // 調低上限時立刻刪掉多出來的舊紀錄；關閉「保存密碼」時把已保存的密碼一併刪掉
        let mut changed = false;
        if next.history_limit < prev.history_limit {
            let _ = store.trim(next.history_limit);
            changed = true;
        }
        if prev.history_secrets && !next.history_secrets {
            let _ = store.delete_category(Category::Secret);
            changed = true;
        }
        if changed {
            let _ = app.emit("history:changed", ());
        }
    }
    if let Some(cloud) = app.try_state::<Cloud>() {
        cloud.apply_settings(&next);
    }
    if let Err(e) = next.save(&state.settings_path) {
        eprintln!("copyc: 無法儲存設定：{e}");
    }
    tray::refresh(app);
    crate::settings_window::retitle(app);
    let _ = app.emit("settings:changed", &next);
    next
}

/* ---------------- 雲端同步 ---------------- */

#[tauri::command]
pub fn cloud_status(cloud: tauri::State<'_, Cloud>) -> cloud::Status {
    cloud.status()
}

/// 開瀏覽器登入 Google。`page` 是登入完成後瀏覽器分頁上顯示的訊息（依介面語言）。
#[tauri::command]
pub async fn cloud_sign_in(app: AppHandle, page: String) -> Result<(), CloudError> {
    let page: String = page.chars().take(200).collect();
    app.state::<Cloud>().sign_in(&page).await
}

#[tauri::command]
pub async fn cloud_sign_out(app: AppHandle) -> Result<(), CloudError> {
    app.state::<Cloud>().sign_out().await;
    Ok(())
}

#[tauri::command]
pub fn cloud_sync_now(cloud: tauri::State<'_, Cloud>) {
    cloud.sync_now();
}

/// 刪除 Google Drive 裡 CopyC 的所有資料並登出。
#[tauri::command]
pub async fn cloud_delete_all(app: AppHandle) -> Result<(), CloudError> {
    app.state::<Cloud>().delete_all().await
}

/* ---------------- 本機歷史紀錄 ---------------- */

#[tauri::command]
pub fn history_query(
    state: tauri::State<'_, AppState>,
    q: String,
    category: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<Page, String> {
    let Some(store) = state.store.get() else {
        return Ok(Page { items: Vec::new(), total: 0, counts: Default::default() });
    };
    let mut page = store.query(&q, category.as_deref(), offset, limit.min(200)).map_err(|e| e.to_string())?;
    // 隱私打碼模式：分享螢幕時連歷史列表也只顯示類型
    if state.settings.lock().privacy_mode {
        for item in &mut page.items {
            item.payload = serde_json::json!({ "kind": "private" });
            item.text = None;
            item.paths.clear();
            item.source = None;
        }
    }
    Ok(page)
}

/// 把一筆歷史寫回剪貼簿。回傳 "ok"、"missing"（檔案都不存在）或 "unavailable"（圖片或超長文字）。
#[tauri::command]
pub fn history_copy(state: tauri::State<'_, AppState>, id: i64) -> Result<&'static str, String> {
    let Some(store) = state.store.get() else { return Ok("unavailable") };
    let Some(content) = store.content(id).map_err(|e| e.to_string())? else { return Ok("unavailable") };
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    match content {
        Content::Text(text) => clipboard.set_text(text).map_err(|e| e.to_string())?,
        Content::Files(paths) => {
            let existing: Vec<_> = paths.into_iter().filter(|p| p.exists()).collect();
            if existing.is_empty() {
                return Ok("missing");
            }
            clipboard.set().file_list(&existing).map_err(|e| e.to_string())?;
        }
    }
    Ok("ok")
}

#[tauri::command]
pub fn history_delete(app: AppHandle, state: tauri::State<'_, AppState>, id: i64) -> Result<(), String> {
    if let Some(store) = state.store.get() {
        store.delete(id).map_err(|e| e.to_string())?;
        let _ = app.emit("history:changed", ());
    }
    Ok(())
}

#[tauri::command]
pub fn history_clear(app: AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    if let Some(store) = state.store.get() {
        store.clear().map_err(|e| e.to_string())?;
        let _ = app.emit("history:changed", ());
    }
    Ok(())
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
    open_site(&state, "download");
}

/// Give me a Kopi：打開官網的贊助區塊（MYR 用 DuitNow QR，其他貨幣用 GitHub Sponsors）。
#[tauri::command]
pub fn open_kopi(state: tauri::State<'_, AppState>) {
    open_site(&state, "kopi");
}

/// 官網只有中文與英文版：中文介面開中文頁，其他語言開英文頁。
fn open_site(state: &AppState, anchor: &str) {
    let lang = i18n::resolve(state.settings.lock().language);
    let base = match lang {
        i18n::Lang::Zh | i18n::Lang::ZhHans => "https://www.copyc.app.dreamsworld.top/",
        _ => "https://www.copyc.app.dreamsworld.top/en/",
    };
    platform::open_url(&format!("{base}#{anchor}"));
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
        entry: None,
        share: None,
        via: None,
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
