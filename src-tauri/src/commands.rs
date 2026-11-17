//! 前端可呼叫的指令。

use crate::{
    autostart::{self, Autostart},
    classify::{Category, Clip, Payload, SensitiveReason},
    cloud::{self, transfer, Cloud, CloudError},
    history::{Content, Page},
    hud, i18n, monitor, platform,
    settings::Settings,
    state::{AppState, HistoryOut, Stats},
    tray,
};
use serde::Serialize;
use std::{path::PathBuf, thread, time::Duration};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

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

/* ---------------- 檔案互傳 ---------------- */

#[tauri::command]
pub fn transfer_status(app: AppHandle) -> transfer::Status {
    transfer::status(&app)
}

/// 傳送拖進偏好設定的檔案或資料夾，回傳排進去的檔案數。
#[tauri::command]
pub fn transfer_send(app: AppHandle, paths: Vec<String>, peer: String) -> usize {
    transfer::send_paths(&app, paths.into_iter().map(PathBuf::from).collect(), peer)
}

/// 開檔案選擇視窗，選好就傳。
#[tauri::command]
pub fn transfer_pick(app: AppHandle, peer: String) {
    let handle = app.clone();
    app.dialog().file().set_title(i18n::strings(lang(&app)).send_files).pick_files(move |picked| {
        let paths: Vec<PathBuf> = picked.unwrap_or_default().into_iter().filter_map(|p| p.into_path().ok()).collect();
        if !paths.is_empty() {
            transfer::send_paths(&handle, paths, peer);
        }
    });
}

/// 傳送剪貼簿裡的檔案或圖片；回傳 0 表示剪貼簿裡沒有可以傳的東西。
#[tauri::command]
pub async fn transfer_clipboard(app: AppHandle, peer: String) -> usize {
    tauri::async_runtime::spawn_blocking(move || transfer::send_clipboard(&app, peer)).await.unwrap_or(0)
}

#[tauri::command]
pub fn transfer_cancel(app: AppHandle, id: u64) {
    app.state::<Cloud>().transfers.cancel(&app, id);
}

#[tauri::command]
pub fn transfer_clear(app: AppHandle) {
    app.state::<Cloud>().transfers.clear_finished(&app);
}

/// 打開收到的檔案（`reveal` 為 true 時改成在檔案總管 / Finder 裡顯示）。
#[tauri::command]
pub fn transfer_open(app: AppHandle, id: u64, reveal: bool) -> bool {
    let Some(path) = transfer::item(&app, id).and_then(|it| it.local_path()) else { return false };
    if reveal {
        transfer::reveal(&path);
    } else {
        transfer::open_path(&path);
    }
    true
}

#[tauri::command]
pub fn transfer_open_dir(app: AppHandle) {
    let dir = transfer::dest_dir(&app);
    let _ = std::fs::create_dir_all(&dir);
    transfer::open_path(&dir);
}

/// 選擇收到的檔案要存在哪裡。
#[tauri::command]
pub fn transfer_pick_dir(app: AppHandle) {
    let handle = app.clone();
    app.dialog().file().set_directory(transfer::dest_dir(&app)).pick_folder(move |picked| {
        let Some(dir) = picked.and_then(|p| p.into_path().ok()) else { return };
        let mut next = handle.state::<AppState>().settings.lock().clone();
        next.transfer_dir = dir.to_string_lossy().into_owned();
        commit_settings(&handle, next);
        let _ = handle.emit("transfer:changed", ());
    });
}

/// 下載其他裝置經由 Google Drive 傳來的檔案。超過 24 小時回傳 "expired"。
#[tauri::command]
pub async fn transfer_download(app: AppHandle, mid: String) -> Result<(), String> {
    let handle = app.clone();
    // 下載可能很久：背景進行，進度在清單裡；這裡只先確認還沒過期
    let entry_ok = transfer::inbox_ready(&app, &mid);
    if let Err(code) = entry_ok {
        return Err(code.into());
    }
    tauri::async_runtime::spawn(async move {
        let _ = transfer::download(&handle, &mid).await;
    });
    Ok(())
}

/// 打開（或在資料夾裡顯示）下載好的檔案。
#[tauri::command]
pub fn transfer_inbox_open(app: AppHandle, mid: String, reveal: bool) -> bool {
    let Some(path) = transfer::inbox_file(&app, &mid) else { return false };
    if reveal {
        transfer::reveal(&path);
    } else {
        transfer::open_path(&path);
    }
    true
}

/// 通知框的「發送到…」：列出可以傳的裝置。
#[tauri::command]
pub fn hud_peers(app: AppHandle) -> Vec<transfer::Peer> {
    transfer::peer_list(&app)
}

/// 通知框的「發送到…」：把剛複製的檔案或圖片傳給選好的裝置，回傳排進去的檔案數。
#[tauri::command]
pub async fn hud_send(app: AppHandle, peer: String) -> usize {
    tauri::async_runtime::spawn_blocking(move || transfer::send_clipboard(&app, peer)).await.unwrap_or(0)
}

fn lang(app: &AppHandle) -> i18n::Lang {
    i18n::resolve(app.state::<AppState>().settings.lock().language)
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
    /// 新版本已經在背景下載好，可以直接重新啟動更新
    update_ready: bool,
    /// 這台電腦的型號（目前只有 Mac 會有），例如「MacBook Pro (Mac17,2)」
    device: Option<String>,
    /// 內建螢幕頂端：island（靈動島）、notch（瀏海）
    camera: Option<&'static str>,
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
        update_ready: app.state::<crate::updater::Updater>().is_ready(),
        device: platform::device_model_cached(),
        camera: platform::camera_kind(&app),
    }
}

/// 安裝已經下載好的新版本並重新啟動；回傳 false 表示沒有可安裝的（頁面改為打開官網下載）。
#[tauri::command]
pub fn update_install(app: AppHandle) -> bool {
    crate::updater::install(&app)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteData {
    notices: Vec<crate::remote::NoticeOut>,
    /// 語言 → 文字鍵 → 取代的文字（頁面會再檢查只取代現有的純文字）
    strings: std::collections::HashMap<String, std::collections::HashMap<String, String>>,
}

/// 遠端資料裡給頁面用的部分：目前要顯示的公告與文字修正。
#[tauri::command]
pub fn remote_data(app: AppHandle, state: tauri::State<'_, AppState>) -> RemoteData {
    let lang = i18n::resolve(state.settings.lock().language);
    let lang = serde_json::to_value(lang).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    let version = app.package_info().version.to_string();
    let strings = ["zh-Hant", "zh-HK", "zh-Hans", "en", "ja", "ko", "fr", "ms"]
        .into_iter()
        .map(|l| (l.to_string(), crate::remote::strings(l)))
        .filter(|(_, m)| !m.is_empty())
        .collect();
    RemoteData {
        notices: crate::remote::notices(&lang, platform::PLATFORM, platform::is_packaged(), &version),
        strings,
    }
}

/// 公告的「了解更多」：網址來自簽章過的遠端資料，頁面只傳公告 id。
#[tauri::command]
pub fn open_notice(id: String) {
    if let Some(url) = crate::remote::notice_url(&id) {
        platform::open_url(&url);
    }
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
        i18n::Lang::Zh | i18n::Lang::ZhHk | i18n::Lang::ZhHans => "https://www.copyc.app.dreamsworld.top/",
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
