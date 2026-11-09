//! Android 版的同步核心：登入狀態、加密金鑰與即時同步連線。
//! 加密、線路格式與 Drive 存取直接沿用電腦版的原始碼（#[path]），兩邊才不會不一致。

use crate::auth;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, Notify};

#[path = "../../../src-tauri/src/cloud/crypto.rs"]
pub mod crypto;
// 電腦版的 Drive 模組還有上傳備份用的功能，Android 版目前只用來讀取金鑰
#[allow(dead_code)]
#[path = "../../../src-tauri/src/cloud/drive.rs"]
pub mod drive;
#[path = "../../../src-tauri/src/cloud/wire.rs"]
pub mod wire;

use crypto::Key;
use drive::Drive;

pub const RELAY_URL: &str = "wss://www.copyc.app.dreamsworld.top/sync/ws";
const KEY_FILE: &str = "copyc-key-v1";
const ACCOUNT_FILE: &str = "cloud.json";
const DEVICE_FILE: &str = "device-id";
const PING: Duration = Duration::from_secs(30);
/// 即時同步一次最多傳這麼大的文字（和電腦版相同）。
pub const LIVE_MAX_BYTES: usize = 64 * 1024;
const SEEN_MAX: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CloudError {
    Unavailable,
    Network,
    Cancelled,
    SignedOut,
    RateLimited,
    Offline,
    /// 授權畫面出錯（例如 Google Cloud 的 Android 用戶端還沒設定好）
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    SignInFailed,
    TooLong,
}

impl From<reqwest::Error> for CloudError {
    fn from(_: reqwest::Error) -> Self {
        CloudError::Network
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Live {
    #[default]
    Off,
    Connecting,
    On,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Account {
    email: String,
    sub: String,
}

struct Session {
    access: String,
    access_exp: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    available: bool,
    signed_in: bool,
    email: Option<String>,
    device_name: String,
    live: Live,
    signing_in: bool,
    error: Option<CloudError>,
}

/// 收到或送出的一則內容（給畫面顯示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipEvent {
    pub text: String,
    pub device: String,
    pub at: u64,
    pub incoming: bool,
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub struct Cloud {
    app: AppHandle,
    dir: PathBuf,
    pub http: reqwest::Client,
    device_id: String,
    device_name: String,
    account: Mutex<Option<Account>>,
    session: tokio::sync::Mutex<Option<Session>>,
    key: Mutex<Option<Key>>,
    live: Mutex<Live>,
    error: Mutex<Option<CloudError>>,
    relay_tx: Mutex<Option<mpsc::UnboundedSender<String>>>,
    /// 登入狀態變了、或 App 回到前景：叫醒即時同步連線
    wake: Notify,
    /// 正在顯示 Google 的授權畫面
    signing_in: Mutex<bool>,
    seen: Mutex<VecDeque<u64>>,
    /// 最後一次從其他電腦收到並放進剪貼簿的內容，避免又傳回去
    last_received: Mutex<Option<String>>,
}

impl Cloud {
    pub fn new(app: AppHandle, dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let account = std::fs::read_to_string(dir.join(ACCOUNT_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            ;
        let device_id = std::fs::read_to_string(dir.join(DEVICE_FILE))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| s.len() == 16 && s.chars().all(|c| c.is_ascii_hexdigit()))
            .unwrap_or_else(|| {
                let id = format!("{:016x}", rand::random::<u64>());
                let _ = std::fs::write(dir.join(DEVICE_FILE), &id);
                id
            });
        Self {
            app,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .user_agent(concat!("CopyC-Android/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            device_id,
            device_name: "Android".into(),
            account: Mutex::new(account),
            session: tokio::sync::Mutex::new(None),
            key: Mutex::new(None),
            live: Mutex::new(Live::Off),
            error: Mutex::new(None),
            relay_tx: Mutex::new(None),
            wake: Notify::new(),
            signing_in: Mutex::new(false),
            seen: Mutex::new(VecDeque::new()),
            last_received: Mutex::new(None),
            dir,
        }
    }

    pub fn status(&self) -> Status {
        let account = self.account.lock().clone();
        Status {
            available: auth::available(&self.app),
            signed_in: account.is_some(),
            email: account.map(|a| a.email),
            device_name: self.device_name.clone(),
            live: *self.live.lock(),
            signing_in: *self.signing_in.lock(),
            error: *self.error.lock(),
        }
    }

    fn changed(&self) {
        let _ = self.app.emit("status", ());
    }

    fn set_live(&self, live: Live) {
        *self.live.lock() = live;
        self.changed();
    }

    fn set_error(&self, error: Option<CloudError>) {
        *self.error.lock() = error;
        self.changed();
    }

    /// App 回到前景：連線可能在背景被系統切斷了，馬上重連。
    pub fn resume(&self) {
        self.wake.notify_one();
    }

    /* ---------------- 登入 ---------------- */

    /// 登入：顯示 Google 的帳號選擇與同意畫面，拿到 access token 後讀取帳號資訊。
    pub async fn sign_in(&self) -> Result<(), CloudError> {
        {
            let mut busy = self.signing_in.lock();
            if *busy {
                return Ok(());
            }
            *busy = true;
        }
        self.set_error(None);
        let result = self.sign_in_inner().await;
        *self.signing_in.lock() = false;
        self.set_error(result.err().filter(|e| *e != CloudError::Cancelled));
        self.wake.notify_one();
        result
    }

    async fn sign_in_inner(&self) -> Result<(), CloudError> {
        let token = auth::authorize(&self.app, true).await?;
        let info = auth::token_info(&self.http, &token).await?;
        *self.session.lock().await =
            Some(Session { access: token, access_exp: now_ms() + info.expires_in_secs() * 1000 });
        *self.key.lock() = None;
        let account = Account { email: info.email, sub: info.sub };
        let _ = std::fs::write(self.dir.join(ACCOUNT_FILE), serde_json::to_vec(&account).unwrap_or_default());
        *self.account.lock() = Some(account);
        Ok(())
    }

    pub async fn sign_out(&self) {
        if let Some(s) = self.session.lock().await.take() {
            auth::revoke(&self.http, &s.access).await;
        }
        let _ = std::fs::remove_file(self.dir.join(ACCOUNT_FILE));
        *self.account.lock() = None;
        *self.key.lock() = None;
        self.set_error(None);
        self.wake.notify_one();
    }

    fn signed_in(&self) -> bool {
        self.account.lock().is_some()
    }

    /* ---------------- Token 與金鑰 ---------------- */

    /// 目前有效的 access token；快到期時在背景向 Google 要一個新的（不會跳出畫面）。
    async fn access_token(&self) -> Result<String, CloudError> {
        let mut guard = self.session.lock().await;
        if let Some(s) = guard.as_ref().filter(|s| s.access_exp > now_ms() + 120_000) {
            return Ok(s.access.clone());
        }
        let token = auth::authorize(&self.app, false).await?;
        let info = auth::token_info(&self.http, &token).await?;
        *guard = Some(Session { access: token.clone(), access_exp: now_ms() + info.expires_in_secs() * 1000 });
        Ok(token)
    }

    /// 從 Drive 讀取電腦版放的金鑰；這是第一台裝置時產生一把並上傳（和電腦版規則相同：以最早建立的為準）。
    async fn key(&self) -> Result<Key, CloudError> {
        if let Some(k) = self.key.lock().clone() {
            return Ok(k);
        }
        let token = self.access_token().await?;
        let drive = Drive { http: &self.http, token: &token };
        let mut files = drive.list().await?;
        if !files.iter().any(|f| f.name == KEY_FILE) {
            let body = serde_json::json!({ "v": 1, "key": Key::generate().to_base64() }).to_string();
            drive.create(KEY_FILE, body.into_bytes()).await?;
            files = drive.list().await?;
        }
        let mut keys: Vec<_> = files.iter().filter(|f| f.name == KEY_FILE).collect();
        keys.sort_by(|a, b| (&a.created_time, &a.id).cmp(&(&b.created_time, &b.id)));
        #[derive(Deserialize)]
        struct KeyFile {
            key: String,
        }
        let raw = drive.download(&keys.first().ok_or(CloudError::Network)?.id).await?;
        let key = serde_json::from_slice::<KeyFile>(&raw)
            .ok()
            .and_then(|k| Key::from_base64(&k.key))
            .ok_or(CloudError::Network)?;
        *self.key.lock() = Some(key.clone());
        Ok(key)
    }

    /* ---------------- 即時同步 ---------------- */

    /// 把文字傳到其他電腦。
    pub fn send(&self, text: &str) -> Result<ClipEvent, CloudError> {
        if text.len() > LIVE_MAX_BYTES {
            return Err(CloudError::TooLong);
        }
        let key = self.key.lock().clone().ok_or(CloudError::Offline)?;
        let tx = self.relay_tx.lock().clone().ok_or(CloudError::Offline)?;
        let (id, data) = wire::seal(&key, text, &self.device_name);
        self.first_time(id);
        tx.send(wire::wire(&data, &[])).map_err(|_| CloudError::Offline)?;
        Ok(ClipEvent { text: text.to_string(), device: self.device_name.clone(), at: now_ms(), incoming: false })
    }

    /// 這段文字是不是剛從其他電腦收到的（傳回去沒有意義）。
    pub fn is_last_received(&self, text: &str) -> bool {
        self.last_received.lock().as_deref() == Some(text)
    }

    fn first_time(&self, id: u64) -> bool {
        if id == 0 {
            return true;
        }
        let mut seen = self.seen.lock();
        if seen.contains(&id) {
            return false;
        }
        if seen.len() == SEEN_MAX {
            seen.pop_front();
        }
        seen.push_back(id);
        true
    }

    fn receive(&self, key: &Key, msg: &str) -> bool {
        let Some(clip) = wire::decode(key, msg) else { return false };
        if !self.first_time(clip.id) || clip.text.trim().is_empty() {
            return true;
        }
        *self.last_received.lock() = Some(clip.text.clone());
        {
            use tauri_plugin_clipboard_manager::ClipboardExt;
            let _ = self.app.clipboard().write_text(clip.text.clone());
        }
        let _ = self.app.emit("clip", ClipEvent { text: clip.text, device: clip.device, at: clip.at, incoming: true });
        true
    }
}

/// 背景工作：登入後保持一條即時同步連線，斷線就重連。
pub async fn run(app: AppHandle) {
    let cloud = app.state::<Cloud>();
    let mut backoff = Duration::from_secs(2);
    loop {
        if !cloud.signed_in() {
            cloud.set_live(Live::Off);
            cloud.wake.notified().await;
            continue;
        }
        cloud.set_live(Live::Connecting);
        let wait = match connect(&cloud).await {
            Ok(()) => {
                backoff = Duration::from_secs(2);
                Duration::from_millis(300)
            }
            Err(CloudError::SignedOut) => {
                cloud.sign_out().await;
                cloud.set_error(Some(CloudError::SignedOut));
                continue;
            }
            Err(CloudError::Unavailable) => Duration::from_secs(600),
            Err(_) => {
                backoff = (backoff * 2).min(Duration::from_secs(60));
                backoff
            }
        };
        *cloud.relay_tx.lock() = None;
        if cloud.signed_in() {
            cloud.set_live(Live::Connecting);
        }
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = cloud.wake.notified() => {}
        }
    }
}

async fn connect(cloud: &Cloud) -> Result<(), CloudError> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, Message};

    let key = cloud.key().await?;
    // 中繼伺服器向 Google 確認這個 access token 屬於哪個帳號（電腦版送的是 id_token）
    let token = cloud.access_token().await?;
    let mut req = format!("{RELAY_URL}?d={}", cloud.device_id).into_client_request().map_err(|_| CloudError::Network)?;
    req.headers_mut()
        .insert("Authorization", format!("Bearer {token}").parse().map_err(|_| CloudError::Network)?);
    let (ws, _) = match tokio_tungstenite::connect_async(req).await {
        Ok(ok) => ok,
        Err(tungstenite::Error::Http(resp)) => {
            return Err(match resp.status().as_u16() {
                401 => {
                    if let Some(s) = cloud.session.lock().await.as_mut() {
                        s.access_exp = 0;
                    }
                    CloudError::Network
                }
                404 | 503 => CloudError::Unavailable,
                429 => CloudError::RateLimited,
                _ => CloudError::Network,
            })
        }
        Err(_) => return Err(CloudError::Network),
    };
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    *cloud.relay_tx.lock() = Some(tx);
    cloud.set_error(None);
    cloud.set_live(Live::On);
    let mut ping = tokio::time::interval(PING);
    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(text))) if text != "pong" => {
                    if !cloud.receive(&key, &text) {
                        // 解不開：金鑰可能換了，重新讀取後再連
                        *cloud.key.lock() = None;
                        return Ok(());
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Ok(()),
                _ => {}
            },
            out = rx.recv() => match out {
                Some(out) => if sink.send(Message::Text(out)).await.is_err() { return Ok(()) },
                None => return Ok(()),
            },
            _ = ping.tick() => if sink.send(Message::Text("ping".into())).await.is_err() { return Ok(()) },
            _ = cloud.wake.notified() => {
                // 登出，或 App 回到前景時確認連線還活著
                if !cloud.signed_in() {
                    let _ = sink.send(Message::Close(None)).await;
                    return Ok(());
                }
                if sink.send(Message::Text("ping".into())).await.is_err() { return Ok(()) }
            }
        }
    }
}
