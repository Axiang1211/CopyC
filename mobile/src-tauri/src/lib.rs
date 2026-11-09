//! CopyC Android 版（第 1 階段原型）：用 Google 登入、App 開著時收到電腦傳來的內容就放進剪貼簿，
//! 以及把手機的剪貼簿（或從分享選單傳來的文字）傳到電腦。

mod auth;
mod cloud;

use cloud::{ClipEvent, Cloud, CloudError, Status};
use serde::Serialize;
use tauri::{Manager, State};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignIn {
    user_code: String,
    verification_url: String,
    expires_in: u64,
}

#[tauri::command]
fn status(cloud: State<'_, Cloud>) -> Status {
    cloud.status()
}

/// 開始登入：回傳要在 google.com/device 輸入的代碼，並把代碼放進剪貼簿方便貼上。
#[tauri::command]
async fn sign_in_start(app: tauri::AppHandle) -> Result<SignIn, CloudError> {
    let cloud = app.state::<Cloud>();
    let code = cloud.sign_in_start().await?;
    {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        let _ = app.clipboard().write_text(code.user_code.clone());
    }
    Ok(SignIn { user_code: code.user_code, verification_url: code.verification_url, expires_in: code.expires_in })
}

#[tauri::command]
fn sign_in_cancel(cloud: State<'_, Cloud>) {
    cloud.sign_in_cancel();
}

/// 用瀏覽器打開 Google 的輸入代碼頁面（只接受 Google 的網址）。
#[tauri::command]
fn open_verification(app: tauri::AppHandle, url: String) {
    let ok = url.starts_with("https://www.google.com/") || url.starts_with("https://google.com/");
    if ok {
        use tauri_plugin_opener::OpenerExt;
        let _ = app.opener().open_url(url, None::<&str>);
    }
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
fn send_text(cloud: State<'_, Cloud>, text: String) -> Result<Sent, CloudError> {
    if text.trim().is_empty() {
        return Ok(Sent { clip: None, echo: false });
    }
    if cloud.is_last_received(&text) {
        return Ok(Sent { clip: None, echo: true });
    }
    Ok(Sent { clip: Some(cloud.send(&text)?), echo: false })
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
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            status,
            sign_in_start,
            sign_in_cancel,
            open_verification,
            sign_out,
            read_clipboard,
            write_clipboard,
            send_text,
            resume,
        ])
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            app.manage(Cloud::new(app.handle().clone(), dir));
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move { cloud::run(handle).await });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("無法啟動 CopyC");
}
