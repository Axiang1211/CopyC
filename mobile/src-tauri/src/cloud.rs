//! Android 版的同步核心：登入狀態、加密金鑰與即時同步連線。
//! 加密、線路格式與 Drive 存取直接沿用電腦版的原始碼（#[path]），兩邊才不會不一致。

use crate::{android, auth};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, Notify};

#[path = "../../../src-tauri/src/cloud/crypto.rs"]
pub mod crypto;
// 電腦版的 Drive 模組還有上傳備份用的功能，Android 版目前只用來讀取金鑰
#[allow(dead_code)]
#[path = "../../../src-tauri/src/cloud/drive.rs"]
pub mod drive;
#[path = "../../../src-tauri/src/cloud/lan_proto.rs"]
pub mod lan_proto;
#[path = "../../../src-tauri/src/cloud/texthash.rs"]
pub mod texthash;
#[path = "../../../src-tauri/src/cloud/wire.rs"]
pub mod wire;

use crypto::Key;
use drive::Drive;

pub const RELAY_URL: &str = "wss://www.copyc.app.dreamsworld.top/sync/ws";
const KEY_FILE: &str = "copyc-key-v1";
const ACCOUNT_FILE: &str = "cloud.json";
const DEVICE_FILE: &str = "device-id";
const PREFS_FILE: &str = "prefs.json";
const PING: Duration = Duration::from_secs(30);
/// 送出前，連線至少要在這段時間內收過伺服器的回應，才算確定還活著
const FRESH: Duration = Duration::from_secs(20);
/// 送出 ping 後這麼久沒有回應，就當作連線已經斷了（手機在背景時連線常被系統悄悄切斷）
const PONG_WAIT: Duration = Duration::from_secs(4);
/// 等連線確認或重新連上的上限
const SEND_WAIT: Duration = Duration::from_secs(12);
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

/// 使用者在 App 裡的開關。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    /// 背景接收：常駐通知讓 App 在背景時也保持連線
    pub background: bool,
    /// 同一個網路直接互傳（Android 17 起需要「附近裝置」權限，使用者開啟時才詢問）
    pub lan: bool,
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
    prefs: Prefs,
    /// 區域網路裡找到幾台同帳號的電腦
    lan_peers: usize,
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
    pub(crate) device_id: String,
    pub(crate) device_name: String,
    account: Mutex<Option<Account>>,
    session: tokio::sync::Mutex<Option<Session>>,
    pub(crate) key: Mutex<Option<Key>>,
    live: Mutex<Live>,
    error: Mutex<Option<CloudError>>,
    relay_tx: Mutex<Option<mpsc::UnboundedSender<String>>>,
    /// 登入狀態變了、或 App 回到前景：叫醒即時同步連線
    wake: Notify,
    /// 最後一次收到中繼伺服器任何回應（訊息或 pong）的時間
    last_seen: Mutex<Instant>,
    /// 連線確認還活著（收到 pong）或剛重新連上
    alive: Notify,
    /// 正在顯示 Google 的授權畫面
    signing_in: Mutex<bool>,
    seen: Mutex<VecDeque<u64>>,
    /// 最後一次從其他電腦收到並放進剪貼簿的內容，避免又傳回去
    last_received: Mutex<Option<String>>,
    prefs: Mutex<Prefs>,
    /// 畫面在前景（看得到 App）；不在前景時收到內容會跳通知
    foreground: AtomicBool,
    pub(crate) lan: lan_proto::Lan,
    /// 區域網路直傳的開關或登入狀態變了
    pub(crate) lan_wake: Notify,
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
        let prefs = std::fs::read_to_string(dir.join(PREFS_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            app,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .user_agent(concat!("CopyC-Android/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            device_id,
            device_name: android::device_name().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Android".into()),
            account: Mutex::new(account),
            session: tokio::sync::Mutex::new(None),
            key: Mutex::new(None),
            live: Mutex::new(Live::Off),
            error: Mutex::new(None),
            relay_tx: Mutex::new(None),
            wake: Notify::new(),
            last_seen: Mutex::new(Instant::now().checked_sub(FRESH * 2).unwrap_or_else(Instant::now)),
            alive: Notify::new(),
            signing_in: Mutex::new(false),
            seen: Mutex::new(VecDeque::new()),
            last_received: Mutex::new(None),
            prefs: Mutex::new(prefs),
            foreground: AtomicBool::new(true),
            lan: lan_proto::Lan::default(),
            lan_wake: Notify::new(),
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
            prefs: *self.prefs.lock(),
            lan_peers: self.lan.count(),
        }
    }

    pub(crate) fn changed(&self) {
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
        self.foreground.store(true, Ordering::Relaxed);
        self.wake.notify_one();
        self.lan_wake.notify_one();
    }

    /// 畫面離開前景：之後收到的內容改用通知告訴使用者。
    pub fn pause(&self) {
        self.foreground.store(false, Ordering::Relaxed);
    }

    pub fn set_prefs(&self, prefs: Prefs) {
        *self.prefs.lock() = prefs;
        let _ = std::fs::write(self.dir.join(PREFS_FILE), serde_json::to_vec(&prefs).unwrap_or_default());
        self.apply_background();
        self.lan_wake.notify_one();
        self.changed();
    }

    pub(crate) fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    pub(crate) fn prefs(&self) -> Prefs {
        *self.prefs.lock()
    }

    /// 依設定與登入狀態開關背景服務（App 在前景時呼叫，Android 才允許啟動前景服務）。
    pub fn apply_background(&self) {
        android::set_background(self.prefs().background && self.signed_in());
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
        self.apply_background();
        self.lan_wake.notify_one();
        Ok(())
    }

    pub async fn sign_out(&self) {
        if let Some(s) = self.session.lock().await.take() {
            auth::revoke(&self.http, &s.access).await;
        }
        let _ = std::fs::remove_file(self.dir.join(ACCOUNT_FILE));
        *self.account.lock() = None;
        *self.key.lock() = None;
        let _ = std::fs::remove_file(self.dir.join(crate::history::CACHE_FILE));
        android::set_background(false);
        self.set_error(None);
        self.wake.notify_one();
        self.lan_wake.notify_one();
    }

    pub(crate) fn signed_in(&self) -> bool {
        self.account.lock().is_some()
    }

    /* ---------------- Token 與金鑰 ---------------- */

    /// 目前有效的 access token；快到期時在背景向 Google 要一個新的（不會跳出畫面）。
    pub(crate) async fn access_token(&self) -> Result<String, CloudError> {
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
    pub(crate) async fn key(&self) -> Result<Key, CloudError> {
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

    /// 把文字傳到其他電腦：先直接傳給同一個網路裡的電腦，其餘的再交給中繼轉送。
    pub async fn send(&self, text: &str) -> Result<ClipEvent, CloudError> {
        if text.len() > LIVE_MAX_BYTES {
            return Err(CloudError::TooLong);
        }
        // 還沒拿到金鑰（剛打開 App）：先等即時同步連上，金鑰會在連線時讀取
        let early = if self.key.lock().is_none() { Some(self.ensure_fresh().await) } else { None };
        let key = self.key.lock().clone().ok_or(CloudError::Offline)?;
        let (id, data) = wire::seal(&key, text, &self.device_name);
        self.first_time(id);
        let targets = if self.prefs().lan { self.lan.targets() } else { Vec::new() };
        let delivered = if targets.is_empty() {
            Vec::new()
        } else {
            lan_proto::deliver(&self.lan, targets, wire::wire(&data, &[])).await
        };
        // 手機在背景時連線可能已經被切斷，但看起來還是「已連線」：先確認還活著，不然就等它重新連上
        let relay_ok = match early {
            Some(ok) => ok,
            None => self.ensure_fresh().await,
        };
        let tx = if relay_ok { self.relay_tx.lock().clone() } else { None };
        let relayed = tx.is_some_and(|tx| tx.send(wire::wire(&data, &delivered)).is_ok());
        if !relayed && delivered.is_empty() {
            return Err(CloudError::Offline);
        }
        Ok(ClipEvent { text: text.to_string(), device: self.device_name.clone(), at: now_ms(), incoming: false })
    }

    /// 確認即時同步連線還活著：最近剛收過回應就直接用；否則送 ping 等回應，
    /// 沒有回應就讓連線重連，等新的連線建立。回傳 false 表示等不到可用的連線。
    async fn ensure_fresh(&self) -> bool {
        if *self.live.lock() == Live::On && self.last_seen.lock().elapsed() < FRESH {
            return true;
        }
        let alive = self.alive.notified();
        tokio::pin!(alive);
        alive.as_mut().enable();
        self.wake.notify_one();
        tokio::time::timeout(SEND_WAIT, alive).await.is_ok() && self.relay_tx.lock().is_some()
    }

    fn mark_alive(&self) {
        *self.last_seen.lock() = Instant::now();
        self.alive.notify_waiters();
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

    /// 收到其他電腦的內容（經中繼或區域網路）：放進剪貼簿，App 不在前景時跳通知。回傳 false 表示解不開。
    /// `max_age`：區域網路收到的內容太舊就不採用，避免有人錄下封包之後重播。
    pub(crate) fn receive(&self, key: &Key, msg: &str, max_age: Option<Duration>) -> bool {
        let Some(clip) = wire::decode(key, msg) else { return false };
        if max_age.is_some_and(|age| now_ms().abs_diff(clip.at) > age.as_millis() as u64) {
            return true;
        }
        if !self.first_time(clip.id) || clip.text.trim().is_empty() {
            return true;
        }
        *self.last_received.lock() = Some(clip.text.clone());
        if !android::set_clipboard(&clip.text) {
            use tauri_plugin_clipboard_manager::ClipboardExt;
            let _ = self.app.clipboard().write_text(clip.text.clone());
        }
        if !self.foreground.load(Ordering::Relaxed) {
            android::notify_received(&clip.device, &preview(&clip.text));
        }
        let _ = self.app.emit("clip", ClipEvent { text: clip.text, device: clip.device, at: clip.at, incoming: true });
        true
    }
}

/// 通知裡顯示的開頭（換行換成空白，最多 80 個字）。
fn preview(text: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(80).collect();
    if flat.chars().count() > 80 {
        out.push('…');
    }
    out
}

/// 背景工作：登入後保持一條即時同步連線，斷線就重連。
pub async fn run(app: AppHandle) {
    let cloud = app.state::<Cloud>();
    // 已經開啟背景接收的話，App 一打開就啟動背景服務
    cloud.apply_background();
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
    cloud.mark_alive();
    let mut ping = tokio::time::interval(PING);
    // 送出 ping 之後等回應的期限；過了還沒回應就重連
    let mut pong_deadline: Option<tokio::time::Instant> = None;
    loop {
        let deadline = pong_deadline;
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    pong_deadline = None;
                    cloud.mark_alive();
                    if text != "pong" && !cloud.receive(&key, &text, None) {
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
            _ = ping.tick() => {
                if sink.send(Message::Text("ping".into())).await.is_err() { return Ok(()) }
                pong_deadline.get_or_insert(tokio::time::Instant::now() + PONG_WAIT);
            }
            _ = async { tokio::time::sleep_until(deadline.unwrap()).await }, if deadline.is_some() => {
                // ping 沒有回應：連線其實已經斷了，重新連線
                let _ = sink.send(Message::Close(None)).await;
                return Ok(());
            }
            _ = cloud.wake.notified() => {
                // 登出，或 App 回到前景／要送出內容時確認連線還活著
                if !cloud.signed_in() {
                    let _ = sink.send(Message::Close(None)).await;
                    return Ok(());
                }
                if sink.send(Message::Text("ping".into())).await.is_err() { return Ok(()) }
                pong_deadline.get_or_insert(tokio::time::Instant::now() + PONG_WAIT);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_preview_is_short_and_single_line() {
        assert_eq!(preview("a\n b\tc"), "a b c");
        let long = "字".repeat(100);
        assert_eq!(preview(&long).chars().count(), 81);
    }

    #[test]
    fn prefs_default_off_and_tolerate_missing_fields() {
        let p: Prefs = serde_json::from_str("{}").unwrap();
        assert_eq!(p, Prefs { background: false, lan: false });
        let p: Prefs = serde_json::from_str(r#"{"lan":true}"#).unwrap();
        assert!(p.lan && !p.background);
    }
}
