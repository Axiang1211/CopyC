//! 自動更新（官網版的大版本更新）：開啟後 5 秒與之後每天檢查一次 GitHub Release 的 `latest.json`，
//! 有新版就在背景下載，驗證簽章（公鑰在 tauri.conf.json，私鑰只在 GitHub Secrets）後
//! 在偏好設定顯示「重新啟動並更新」。
//!
//! Microsoft Store 版不能自己換檔案，而商店又不會替換一直在執行的 App（CopyC 常駐在系統匣），
//! 所以改為每天問商店有沒有新版，有就顯示同一張提示卡，按「更新」請商店下載安裝，裝好後自動重新開啟。
//!
//! 舊的 Release 沒有 `latest.json`（或下載失敗）時，退回只查詢最新版本號，提示使用者到官網下載。

use crate::{platform, remote::parse_version, state::AppState};
use parking_lot::Mutex;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

const FIRST_CHECK: Duration = Duration::from_secs(5);
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const LATEST_URL: &str = "https://api.github.com/repos/Axiang1211/CopyC/releases/latest";

/// 已經下載好、等待使用者按「重新啟動並更新」的版本。
#[derive(Default)]
pub struct Updater {
    ready: Mutex<Option<(Update, Vec<u8>)>>,
    /// 正在下載（每天的自動檢查與「立即檢查」不會重複下載）
    downloading: AtomicBool,
}

/// 檢查一次的結果，給偏好設定的「立即檢查」顯示。
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "state", content = "version", rename_all = "camelCase")]
pub enum Checked {
    Latest,
    Found(String),
    Failed,
}

impl Updater {
    pub fn is_ready(&self) -> bool {
        self.ready.lock().is_some()
    }
}

pub fn start(app: &AppHandle) {
    app.manage(Updater::default());
    if platform::is_packaged() {
        platform::restart_after_update();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(FIRST_CHECK).await;
            loop {
                if app.state::<AppState>().settings.lock().check_updates {
                    let _ = check_store(&app).await;
                }
                tokio::time::sleep(EVERY).await;
            }
        });
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if app.state::<AppState>().settings.lock().check_updates {
                let _ = check(&app).await;
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// 偏好設定的「立即檢查」：不管「自動檢查更新」有沒有開，馬上問一次。
pub async fn check_now(app: &AppHandle) -> Checked {
    if platform::is_packaged() {
        check_store(app).await
    } else {
        check(app).await
    }
}

async fn check(app: &AppHandle) -> Checked {
    let ready = app.state::<Updater>().ready.lock().as_ref().map(|(u, _)| u.version.clone());
    if let Some(version) = ready {
        return Checked::Found(version);
    }
    let checked = match app.updater() {
        Ok(updater) => updater.check().await,
        Err(e) => Err(e),
    };
    match checked {
        Ok(Some(update)) => {
            let version = update.version.clone();
            found(app, &version);
            // 在背景下載，按「立即檢查」的人不用等下載完
            if !app.state::<Updater>().downloading.swap(true, Ordering::SeqCst) {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    match update.download(|_, _| {}, || {}).await {
                        Ok(bytes) => {
                            *app.state::<Updater>().ready.lock() = Some((update, bytes));
                            let _ = app.emit("overview:changed", ());
                        }
                        Err(e) => eprintln!("copyc: 無法下載更新：{e}"),
                    }
                    app.state::<Updater>().downloading.store(false, Ordering::SeqCst);
                });
            }
            Checked::Found(version)
        }
        Ok(None) => Checked::Latest,
        Err(e) => {
            eprintln!("copyc: 無法檢查自動更新：{e}");
            match latest_version(app).await {
                Ok(Some(version)) => {
                    found(app, &version);
                    Checked::Found(version)
                }
                Ok(None) => Checked::Latest,
                Err(()) => Checked::Failed,
            }
        }
    }
}

async fn check_store(app: &AppHandle) -> Checked {
    let current = app.package_info().version.to_string();
    match tauri::async_runtime::spawn_blocking(platform::store_update).await {
        Ok(Ok(Some(version))) if parse_version(&version) > parse_version(&current) => {
            found(app, &version);
            Checked::Found(version)
        }
        Ok(Ok(_)) => Checked::Latest,
        Ok(Err(e)) => {
            eprintln!("copyc: 無法向 Microsoft Store 檢查更新：{e}");
            Checked::Failed
        }
        Err(_) => Checked::Failed,
    }
}

/// Microsoft Store 版的「更新」：請商店下載並安裝（商店的視窗掛在偏好設定上）。
/// 安裝時 Windows 會關閉 CopyC，裝好後依 restart_after_update 的登記重新開啟。
/// 商店無法處理時改為打開商店裡的 CopyC 頁面，讓使用者按「更新」。
pub async fn install_from_store(app: &AppHandle) -> bool {
    #[cfg(windows)]
    let owner = app
        .get_webview_window(crate::settings_window::LABEL)
        .and_then(|w| w.hwnd().ok())
        .map_or(0, |h| h.0 as isize);
    #[cfg(not(windows))]
    let owner = {
        let _ = app;
        0
    };
    let result = tauri::async_runtime::spawn_blocking(move || platform::store_install(owner).map_err(|e| e.to_string())).await;
    match result {
        Ok(Ok(done)) => done,
        Ok(Err(e)) => {
            eprintln!("copyc: Microsoft Store 無法更新：{e}");
            platform::open_url(platform::STORE_PAGE);
            false
        }
        Err(_) => false,
    }
}

/// 退回方案：只查詢 GitHub 上最新的版本號。Ok(None) 是已經最新，Err 是查不到。
async fn latest_version(app: &AppHandle) -> Result<Option<String>, ()> {
    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
    }
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("CopyC/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| ())?;
    let resp = http.get(LATEST_URL).header("Accept", "application/vnd.github+json").send().await.map_err(|_| ())?;
    let release: Release = resp.error_for_status().map_err(|_| ())?.json().await.map_err(|_| ())?;
    let version = release.tag_name.trim_start_matches('v').to_string();
    let valid = !version.is_empty() && version.len() <= 20 && version.chars().all(|c| c.is_ascii_digit() || c == '.');
    let current = app.package_info().version.to_string();
    if !valid {
        return Err(());
    }
    Ok((parse_version(&version) > parse_version(&current)).then_some(version))
}

/// 發現新版本：記下來並打開偏好設定（同一個版本只打開一次），由頁面上的提示卡引導更新。
fn found(app: &AppHandle, version: &str) {
    {
        let state = app.state::<AppState>();
        let mut update = state.update.lock();
        if update.as_deref() == Some(version) {
            return;
        }
        *update = Some(version.to_string());
    }
    let _ = app.emit("overview:changed", ());
    crate::settings_window::open(app);
}

/// 安裝已下載的更新並重新啟動。Windows 會由安裝程式關閉並重新開啟 CopyC；macOS 換好 App 後自己重新啟動。
/// 沒有下載好的更新（或安裝失敗）時回傳 false，頁面改為打開官網下載。
pub fn install(app: &AppHandle) -> bool {
    let Some((update, bytes)) = app.state::<Updater>().ready.lock().take() else { return false };
    match update.install(bytes) {
        Ok(()) => {
            app.restart();
        }
        Err(e) => {
            eprintln!("copyc: 無法安裝更新：{e}");
            let _ = app.emit("overview:changed", ());
            false
        }
    }
}
