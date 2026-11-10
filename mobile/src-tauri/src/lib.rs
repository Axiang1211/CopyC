//! CopyC Android 版（第 1 階段原型）：用 Google 登入、App 開著時收到電腦傳來的內容就放進剪貼簿，
//! 以及把手機的剪貼簿（或從分享選單傳來的文字）傳到電腦。

mod android;
mod auth;
mod cloud;
mod history;
mod lan;

use cloud::{ClipEvent, Cloud, CloudError, Prefs, Status};
use serde::Serialize;
use tauri::{Manager, State};

#[tauri::command]
fn status(cloud: State<'_, Cloud>) -> Status {
    cloud.status()
}

/// 登入：跳出 Google 的帳號選擇與同意畫面。
#[tauri::command]
async fn sign_in(app: tauri::AppHandle) -> Result<(), CloudError> {
    app.state::<Cloud>().sign_in().await
}

#[tauri::command]
async fn sign_out(app: tauri::AppHandle) {
    app.state::<Cloud>().sign_out().await;
}

/// 讀手機目前的剪貼簿（App 在前景時 Android 才允許）。
#[tauri::command]
fn read_clipboard(app: tauri::AppHandle) -> Option<String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().read_text().ok().filter(|t| !t.trim().is_empty())
}

/// 把歷史裡的一則內容放回手機的剪貼簿。
#[tauri::command]
fn write_clipboard(app: tauri::AppHandle, text: String) -> bool {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().write_text(text).is_ok()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Sent {
    clip: Option<ClipEvent>,
    /// 這段文字就是剛從電腦收到的，不再傳回去
    echo: bool,
}

#[tauri::command]
async fn send_text(app: tauri::AppHandle, text: String) -> Result<Sent, CloudError> {
    let cloud = app.state::<Cloud>();
    if text.trim().is_empty() {
        return Ok(Sent { clip: None, echo: false });
    }
    if cloud.is_last_received(&text) {
        return Ok(Sent { clip: None, echo: true });
    }
    Ok(Sent { clip: Some(cloud.send(&text).await?), echo: false })
}

/// 背景接收、同一個網路直接互傳的開關。
#[tauri::command]
fn set_prefs(cloud: State<'_, Cloud>, prefs: Prefs) {
    cloud.set_prefs(prefs);
}

/// 畫面離開前景（之後收到的內容改用通知）。
#[tauri::command]
fn pause(cloud: State<'_, Cloud>) {
    cloud.pause();
}

/// 手機上已經有的歷史紀錄（不連網）。
#[tauri::command]
fn history_cached(cloud: State<'_, Cloud>) -> Vec<history::Item> {
    history::cached(&cloud)
}

/// 從 Google Drive 下載各電腦新的備份並合併。
#[tauri::command]
async fn history_refresh(app: tauri::AppHandle) -> Result<Vec<history::Item>, CloudError> {
    history::refresh(&app.state::<Cloud>()).await
}

/// 畫面回到前景時呼叫：重新確認即時同步連線。
#[tauri::command]
fn resume(cloud: State<'_, Cloud>) {
    cloud.resume();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(auth::plugin())
        .invoke_handler(tauri::generate_handler![
            status,
            sign_in,
            sign_out,
            read_clipboard,
            write_clipboard,
            send_text,
            resume,
            pause,
            set_prefs,
            history_cached,
            history_refresh,
        ])
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            app.manage(Cloud::new(app.handle().clone(), dir));
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move { cloud::run(handle).await });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move { lan::run(handle).await });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("無法啟動 CopyC");
}
