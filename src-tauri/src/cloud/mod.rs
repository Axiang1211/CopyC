//! 雲端同步（選用，預設關閉，登入 Google 後才會啟用）：
//!
//! - **歷史備份**（[`sync`]）：這台電腦的文字類紀錄依天加密後存到使用者自己 Google Drive 的
//!   App 專用資料夾，其他裝置下載後合併進自己的歷史紀錄。
//! - **即時同步**（[`relay`]）：複製的文字加密後經 Cloudflare 轉發給同一個 Google 帳號的其他電腦，
//!   對方直接放進剪貼簿。伺服器只轉發、不保存，也解不開內容。
//!
//! 圖片、檔案與密碼類內容永遠不會離開這台電腦。

pub mod auth;
mod crypto;
mod drive;
mod relay;
mod sync;

use crate::{settings::Settings, state::now_ms};
use crypto::Key;
use drive::{Drive, File};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, watch, Notify};

const ACCOUNT_FILE: &str = "cloud.json";
const DEVICE_FILE: &str = "device-id";
const KEY_FILE: &str = "copyc-key-v1";
/// 即時同步一次最多傳這麼大的文字。
pub const LIVE_MAX_BYTES: usize = 64 * 1024;
/// 其他裝置傳來的內容寫進剪貼簿後，這段時間內看到同一份內容就當成同步來的，不再轉發。
const ECHO_WINDOW: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CloudError {
    /// 這個版本沒有設定 Google 用戶端，或中繼伺服器尚未啟用
    Unavailable,
    Network,
    /// 使用者關掉了瀏覽器或拒絕授權
    Cancelled,
    /// 授權失效（在 Google 帳號撤銷、或太久沒用），需要重新登入
    SignedOut,
    RateLimited,
    /// 正在登入中
    Busy,
    /// 登入沒有完成（不會自動重試，請使用者再按一次）
    SignInFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    email: String,
    sub: String,
    /// 下載過的最新檔案修改時間（RFC 3339），之後只下載比它新的
    #[serde(default)]
    last_pull: String,
    #[serde(default)]
    last_sync: Option<u64>,
}

struct Session {
    access: String,
    access_exp: u64,
    id: Option<String>,
    id_exp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Live {
    #[default]
    Off,
    Connecting,
    On,
}

/// 背景工作要不要跑：登入狀態與兩個開關。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Config {
    signed_in: bool,
    backup: bool,
    live: bool,
}

#[derive(Default)]
struct Runtime {
    busy: bool,
    syncing: bool,
    live: Live,
    error: Option<CloudError>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// 這個版本有沒有設定 Google 登入
    available: bool,
    signed_in: bool,
    email: Option<String>,
    device_name: String,
    last_sync: Option<u64>,
    busy: bool,
    syncing: bool,
    live: Live,
    error: Option<CloudError>,
}

struct Incoming {
    hash: u64,
    device: String,
    at: Instant,
}

pub struct Cloud {
    app: AppHandle,
    dir: PathBuf,
    http: reqwest::Client,
    device_id: String,
    device_name: String,
    account: Mutex<Option<Account>>,
    session: tokio::sync::Mutex<Option<Session>>,
    key: Mutex<Option<Key>>,
    runtime: Mutex<Runtime>,
    config: watch::Sender<Config>,
    wake: Notify,
    sync_now: Notify,
    relay_tx: Mutex<Option<mpsc::UnboundedSender<String>>>,
    incoming: Mutex<Option<Incoming>>,
}

impl Cloud {
    pub fn new(app: AppHandle, dir: PathBuf, settings: &Settings) -> Self {
        let account: Option<Account> = std::fs::read_to_string(dir.join(ACCOUNT_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .filter(|_| auth::store::load(&dir).is_some());
        let device_id = std::fs::read_to_string(dir.join(DEVICE_FILE))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| s.len() == 16 && s.chars().all(|c| c.is_ascii_hexdigit()))
            .unwrap_or_else(|| {
                let id = format!("{:016x}", rand::random::<u64>());
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(dir.join(DEVICE_FILE), &id);
                id
            });
        let config = Config { signed_in: account.is_some(), backup: settings.cloud_backup, live: settings.cloud_live };
        Self {
            app,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .user_agent(concat!("CopyC/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            device_name: device_name(),
            device_id,
            dir,
            account: Mutex::new(account),
            session: tokio::sync::Mutex::new(None),
            key: Mutex::new(None),
            runtime: Mutex::new(Runtime::default()),
            config: watch::Sender::new(config),
            wake: Notify::new(),
            sync_now: Notify::new(),
            relay_tx: Mutex::new(None),
            incoming: Mutex::new(None),
        }
    }

    pub fn status(&self) -> Status {
        let account = self.account.lock().clone();
        let rt = self.runtime.lock();
        Status {
            available: auth::CLIENT_ID.is_some_and(|s| !s.is_empty()),
            signed_in: account.is_some(),
            email: account.as_ref().map(|a| a.email.clone()),
            device_name: self.device_name.clone(),
            last_sync: account.and_then(|a| a.last_sync),
            busy: rt.busy,
            syncing: rt.syncing,
            live: rt.live,
            error: rt.error,
        }
    }

    fn changed(&self) {
        let _ = self.app.emit("cloud:changed", ());
    }

    fn update(&self, f: impl FnOnce(&mut Runtime)) {
        f(&mut self.runtime.lock());
        self.changed();
    }

    /// 設定變了（開關）時呼叫，讓背景工作開始或停止。
    pub fn apply_settings(&self, settings: &Settings) {
        self.config.send_if_modified(|c| {
            let next = Config { backup: settings.cloud_backup, live: settings.cloud_live, ..*c };
            std::mem::replace(c, next) != next
        });
    }

    fn set_signed_in(&self, signed_in: bool) {
        self.config.send_modify(|c| c.signed_in = signed_in);
    }

    /// 有新的本機紀錄：稍後同步一次。
    pub fn nudge(&self) {
        if self.config.borrow().signed_in {
            self.wake.notify_one();
        }
    }

    /// 設定頁的「立即同步」。
    pub fn sync_now(&self) {
        self.sync_now.notify_one();
    }

    fn save_account(&self) {
        if let Some(a) = &*self.account.lock() {
            let _ = std::fs::write(self.dir.join(ACCOUNT_FILE), serde_json::to_vec_pretty(a).unwrap_or_default());
        }
    }

    /* ---------------- 登入與登出 ---------------- */

    pub async fn sign_in(&self, page: &str) -> Result<(), CloudError> {
        {
            let mut rt = self.runtime.lock();
            if rt.busy {
                return Err(CloudError::Busy);
            }
            rt.busy = true;
            rt.error = None;
        }
        self.changed();
        let result = self.sign_in_inner(page).await.map_err(|e| match e {
            CloudError::Cancelled | CloudError::Unavailable => e,
            _ => CloudError::SignInFailed,
        });
        self.update(|rt| {
            rt.busy = false;
            rt.error = result.err().filter(|e| *e != CloudError::Cancelled);
        });
        result
    }

    async fn sign_in_inner(&self, page: &str) -> Result<(), CloudError> {
        let tokens = auth::sign_in(&self.http, page).await?;
        let refresh = tokens.refresh_token.clone().ok_or(CloudError::Network)?;
        let claims = tokens.id_token.as_deref().and_then(auth::claims).ok_or(CloudError::Network)?;
        if !auth::store::save(&self.dir, &refresh) {
            return Err(CloudError::Network);
        }
        *self.session.lock().await = Some(session_from(tokens, None));
        *self.key.lock() = None;
        *self.account.lock() =
            Some(Account { email: claims.email, sub: claims.sub, last_pull: String::new(), last_sync: None });
        self.save_account();
        // 這台電腦既有的紀錄都要備份到（可能是新的）帳號
        if let Some(store) = self.app.state::<crate::state::AppState>().store.get() {
            let _ = store.mark_all_unsynced();
        }
        self.set_signed_in(true);
        Ok(())
    }

    /// 登出：撤銷授權、刪掉本機的 token、金鑰，以及其他裝置同步來的紀錄。
    pub async fn sign_out(&self) {
        if let Some(token) = auth::store::load(&self.dir) {
            auth::revoke(&self.http, &token).await;
        }
        self.forget();
    }

    fn forget(&self) {
        auth::store::delete(&self.dir);
        let _ = std::fs::remove_file(self.dir.join(ACCOUNT_FILE));
        *self.account.lock() = None;
        *self.key.lock() = None;
        if let Ok(mut s) = self.session.try_lock() {
            *s = None;
        }
        self.set_signed_in(false);
        if let Some(store) = self.app.state::<crate::state::AppState>().store.get() {
            let _ = store.delete_remote();
        }
        let _ = self.app.emit("history:changed", ());
        self.update(|rt| {
            rt.syncing = false;
            rt.live = Live::Off;
        });
    }

    /// 刪除 Drive 裡 CopyC 的所有資料（所有裝置的備份與金鑰），然後登出。
    pub async fn delete_all(&self) -> Result<(), CloudError> {
        let token = self.access_token().await?;
        let drive = Drive { http: &self.http, token: &token };
        for f in drive.list().await? {
            drive.delete(&f.id).await?;
        }
        self.sign_out().await;
        Ok(())
    }

    /* ---------------- Token ---------------- */

    async fn ensure_session(&self, need_id: bool) -> Result<tokio::sync::MutexGuard<'_, Option<Session>>, CloudError> {
        let mut guard = self.session.lock().await;
        let now = now_ms();
        let fresh = guard.as_ref().is_some_and(|s| {
            s.access_exp > now + 60_000 && (!need_id || (s.id.is_some() && s.id_exp > now + 300_000))
        });
        if !fresh {
            let refresh = auth::store::load(&self.dir).ok_or(CloudError::SignedOut)?;
            match auth::refresh(&self.http, &refresh).await {
                Ok(tokens) => {
                    let prev_id = guard.take().and_then(|s| s.id.map(|id| (id, s.id_exp)));
                    *guard = Some(session_from(tokens, prev_id));
                }
                Err(CloudError::SignedOut) => {
                    drop(guard);
                    self.forget();
                    self.update(|rt| rt.error = Some(CloudError::SignedOut));
                    return Err(CloudError::SignedOut);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(guard)
    }

    async fn access_token(&self) -> Result<String, CloudError> {
        Ok(self.ensure_session(false).await?.as_ref().map(|s| s.access.clone()).unwrap_or_default())
    }

    async fn id_token(&self) -> Result<String, CloudError> {
        self.ensure_session(true).await?.as_ref().and_then(|s| s.id.clone()).ok_or(CloudError::Network)
    }

    /// 中繼伺服器拒絕了 id_token（例如剛好過期）：下次重新取得。
    async fn drop_id_token(&self) {
        if let Some(s) = self.session.lock().await.as_mut() {
            s.id_exp = 0;
        }
    }

    /* ---------------- 加密金鑰 ---------------- */

    /// 從 Drive 讀取金鑰；沒有就產生一把並上傳。兩台裝置同時產生時，以最早建立的那把為準。
    async fn load_key(&self, drive: &Drive<'_>, files: &[File]) -> Result<Key, CloudError> {
        let relisted;
        let files = if files.iter().any(|f| f.name == KEY_FILE) {
            files
        } else {
            let body = serde_json::json!({ "v": 1, "key": Key::generate().to_base64() }).to_string();
            drive.create(KEY_FILE, body.into_bytes()).await?;
            relisted = drive.list().await?;
            &relisted[..]
        };
        let mut keys: Vec<&File> = files.iter().filter(|f| f.name == KEY_FILE).collect();
        keys.sort_by(|a, b| (&a.created_time, &a.id).cmp(&(&b.created_time, &b.id)));
        let chosen = keys.first().ok_or(CloudError::Network)?;
        #[derive(Deserialize)]
        struct KeyFile {
            key: String,
        }
        let raw = drive.download(&chosen.id).await?;
        let key = serde_json::from_slice::<KeyFile>(&raw)
            .ok()
            .and_then(|k| Key::from_base64(&k.key))
            .ok_or(CloudError::Network)?;
        for extra in &keys[1..] {
            let _ = drive.delete(&extra.id).await;
        }

        let previous = self.key.lock().replace(key.clone());
        if previous.is_some_and(|p| p != key) {
            // 金鑰換了（例如另一台裝置刪除了雲端資料後重新開始）：用新金鑰重新上傳
            if let Some(store) = self.app.state::<crate::state::AppState>().store.get() {
                let _ = store.mark_all_unsynced();
            }
        }
        Ok(key)
    }

    async fn key(&self) -> Result<Key, CloudError> {
        if let Some(k) = self.key.lock().clone() {
            return Ok(k);
        }
        let token = self.access_token().await?;
        let drive = Drive { http: &self.http, token: &token };
        let files = drive.list().await?;
        self.load_key(&drive, &files).await
    }

    /* ---------------- 即時同步 ---------------- */

    /// 這台電腦複製了文字：加密後轉發給其他裝置。
    pub fn send_live(&self, text: &str) {
        if text.len() > LIVE_MAX_BYTES || !self.config.borrow().live {
            return;
        }
        let Some(key) = self.key.lock().clone() else { return };
        let Some(tx) = self.relay_tx.lock().clone() else { return };
        let _ = tx.send(relay::encode(&key, text, &self.device_name));
    }

    /// 剪貼簿管線看到一份內容：如果是剛剛從其他裝置寫進來的，回傳那台裝置的名稱。
    pub fn take_incoming(&self, hash: u64) -> Option<String> {
        let mut slot = self.incoming.lock();
        match slot.take() {
            Some(i) if i.hash == hash && i.at.elapsed() < ECHO_WINDOW => Some(i.device),
            Some(i) if i.at.elapsed() < ECHO_WINDOW => {
                *slot = Some(i);
                None
            }
            _ => None,
        }
    }

    fn expect_incoming(&self, hash: u64, device: String) {
        *self.incoming.lock() = Some(Incoming { hash, device, at: Instant::now() });
    }
}

fn session_from(t: auth::Tokens, prev_id: Option<(String, u64)>) -> Session {
    let now = now_ms();
    let (id, id_exp) = match t.id_token.as_deref().and_then(|id| auth::claims(id).map(|c| (id.to_string(), c.exp))) {
        Some((id, exp)) => (Some(id), exp * 1000),
        None => prev_id.map_or((None, 0), |(id, exp)| (Some(id), exp)),
    };
    Session { access: t.access_token, access_exp: now + t.expires_in * 1000, id, id_exp }
}

/// 顯示在其他裝置上的這台電腦名稱。
fn device_name() -> String {
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok();
    #[cfg(target_os = "macos")]
    let name = std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());
    #[cfg(not(any(windows, target_os = "macos")))]
    let name = std::env::var("HOSTNAME").ok();
    name.map(|n| n.trim().chars().take(40).collect::<String>())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Computer".into())
}

/// 啟動背景工作：定期備份 / 合併歷史，以及即時同步連線。
pub fn start(app: &AppHandle) {
    let a = app.clone();
    tauri::async_runtime::spawn(async move { sync::run(a).await });
    let a = app.clone();
    tauri::async_runtime::spawn(async move { relay::run(a).await });
}
