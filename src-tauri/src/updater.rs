//! 自動更新（官網版的大版本更新）：開啟後 5 秒與之後每天檢查一次 GitHub Release 的 `latest.json`，
//! 有新版就在背景下載，驗證簽章（公鑰在 tauri.conf.json，私鑰只在 GitHub Secrets）後
//! 在偏好設定顯示「重新啟動並更新」。Microsoft Store 版由商店更新，不使用這個功能。
//!
//! 舊的 Release 沒有 `latest.json`（或下載失敗）時，退回只查詢最新版本號，提示使用者到官網下載。

use crate::{platform, remote::parse_version, state::AppState};
use parking_lot::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

const FIRST_CHECK: Duration = Duration::from_secs(5);
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const LATEST_URL: &str = "https://api.github.com/repos/Axiang1211/CopyC/releases/latest";

/// 已經下載好、等待使用者按「重新啟動並更新」的版本。
#[derive(Default)]
pub struct Updater {
    ready: Mutex<Option<(Update, Vec<u8>)>>,
}

impl Updater {
    pub fn is_ready(&self) -> bool {
        self.ready.lock().is_some()
    }
}

pub fn start(app: &AppHandle) {
    app.manage(Updater::default());
    if platform::is_packaged() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if app.state::<AppState>().settings.lock().check_updates {
                check(&app).await;
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

async fn check(app: &AppHandle) {
    if app.state::<Updater>().is_ready() {
        return;
    }
    let checked = match app.updater() {
        Ok(updater) => updater.check().await,
        Err(e) => Err(e),
    };
    match checked {
        Ok(Some(update)) => {
            let version = update.version.clone();
            found(app, &version);
            match update.download(|_, _| {}, || {}).await {
                Ok(bytes) => {
                    *app.state::<Updater>().ready.lock() = Some((update, bytes));
                    let _ = app.emit("overview:changed", ());
                }
                Err(e) => eprintln!("copyc: 無法下載更新：{e}"),
            }
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("copyc: 無法檢查自動更新：{e}");
            if let Some(version) = latest_version(app).await {
                found(app, &version);
            }
        }
    }
}

/// 退回方案：只查詢 GitHub 上最新的版本號。
async fn latest_version(app: &AppHandle) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
    }
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("CopyC/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let resp = http.get(LATEST_URL).header("Accept", "application/vnd.github+json").send().await.ok()?;
    let release: Release = resp.error_for_status().ok()?.json().await.ok()?;
    let version = release.tag_name.trim_start_matches('v').to_string();
    let valid = !version.is_empty() && version.len() <= 20 && version.chars().all(|c| c.is_ascii_digit() || c == '.');
    let current = app.package_info().version.to_string();
    (valid && parse_version(&version) > parse_version(&current)).then_some(version)
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
