//! 互傳檔案（Android 版，M2＋M3）：和電腦版同一套協定。
//!
//! - 同一個 Wi-Fi：手機和電腦直接互傳（file_proto，經由區域網路直傳的 TCP 埠）。
//! - 不在同一個網路：經由使用者自己的 Google Drive 傳（drive_xfer），保存 24 小時，收件端按「下載」才下載。
//! - 傳送：從其他 App 的「分享」選單或 App 裡的「選擇檔案」（Kotlin 先把檔案複製到 App 的暫存資料夾）。
//! - 收到的檔案存到「下載/CopyC」（圖片存到「圖片/CopyC」），用通知告訴使用者，點一下就能打開。

use crate::{
    android,
    cloud::{
        drive::Drive,
        drive_xfer::{self as dx, DeviceInfo, Manifest, XferError},
        file_proto::{self as fp, Offer, SendError},
        lan_proto::{CAP_DRIVE, CAP_FILES},
        now_ms,
        wire::{self, FileNote},
        Cloud,
    },
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::{io::AsyncWriteExt, net::TcpStream};

const RETRY_AFTER: [u64; 6] = [2, 4, 8, 15, 30, 60];
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const EMIT_EVERY: Duration = Duration::from_millis(300);
const KEEP_ITEMS: usize = 100;
const REFRESH: Duration = Duration::from_secs(10 * 60);
const DEVICES_EVERY: Duration = Duration::from_secs(30 * 60);
const PUBLISH_EVERY: Duration = Duration::from_secs(6 * 3600);
const INBOX_FILE: &str = "inbox.json";
const INBOX_KEEP_AFTER: u64 = 24 * 3600 * 1000;
/// 「跨網路傳送上限」可選的值（MB），和電腦版相同。
pub const LIMITS_MB: [u64; 7] = [100, 500, 1024, 5120, 20_480, 51_200, 204_800];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Dir {
    Out,
    In,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Via {
    Lan,
    Ble,
    Drive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum State {
    Waiting,
    Active,
    Retrying,
    Done,
    Failed,
    Canceled,
}

impl State {
    fn finished(self) -> bool {
        matches!(self, State::Done | State::Failed | State::Canceled)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: u64,
    dir: Dir,
    via: Via,
    name: String,
    size: u64,
    done: u64,
    peer: String,
    state: State,
    error: Option<String>,
    /// 收到的檔案存好的位置（content:// URI），點一下就能打開
    uri: Option<String>,
    expires: Option<u64>,
}

struct Entry {
    item: Item,
    key: Option<String>,
    cancel: Arc<AtomicBool>,
    emitted: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum InboxState {
    Ready,
    Downloading,
    Done,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InboxEntry {
    mid: String,
    m: Manifest,
    state: InboxState,
    uri: Option<String>,
    error: Option<XferError>,
}

#[derive(Default)]
pub struct Files {
    entries: Mutex<Vec<Entry>>,
    next: AtomicU64,
    devices: Mutex<Vec<DeviceInfo>>,
    devices_at: Mutex<Option<Instant>>,
    published: Mutex<Option<Instant>>,
    refreshed: Mutex<Option<Instant>>,
    inbox: Mutex<Option<Vec<InboxEntry>>>,
    skipped: Mutex<HashSet<String>>,
    wake: tokio::sync::Notify,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    id: String,
    name: String,
    platform: String,
    lan: bool,
    /// 用藍牙連著（只傳小檔案）
    ble: bool,
    remote: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxOut {
    mid: String,
    name: String,
    size: u64,
    from: String,
    expires: u64,
    state: InboxState,
    error: Option<XferError>,
    uri: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    peers: Vec<Peer>,
    items: Vec<Item>,
    inbox: Vec<InboxOut>,
    remote_mb: u64,
}

fn changed(app: &AppHandle) {
    let _ = app.emit("files", ());
}

impl Files {
    fn add(&self, mut item: Item, key: Option<String>) -> (u64, Arc<AtomicBool>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        item.id = id;
        let cancel = Arc::new(AtomicBool::new(false));
        let mut entries = self.entries.lock();
        entries.push(Entry { item, key, cancel: cancel.clone(), emitted: Instant::now() });
        while entries.len() > KEEP_ITEMS {
            match entries.iter().position(|e| e.item.state.finished()) {
                Some(i) => {
                    entries.remove(i);
                }
                None => break,
            }
        }
        (id, cancel)
    }

    fn set(&self, app: &AppHandle, id: u64, f: impl FnOnce(&mut Item)) {
        if let Some(e) = self.entries.lock().iter_mut().find(|e| e.item.id == id) {
            f(&mut e.item);
            e.emitted = Instant::now();
        }
        changed(app);
    }

    fn fail(&self, app: &AppHandle, id: u64, code: &str) {
        self.set(app, id, |it| {
            it.state = State::Failed;
            it.error = Some(code.into());
        });
    }

    fn progress(&self, app: &AppHandle, id: u64, done: u64) {
        let mut emit = false;
        if let Some(e) = self.entries.lock().iter_mut().find(|e| e.item.id == id) {
            e.item.done = done;
            if e.emitted.elapsed() >= EMIT_EVERY {
                e.emitted = Instant::now();
                emit = true;
            }
        }
        if emit {
            changed(app);
        }
    }

    pub fn cancel(&self, app: &AppHandle, id: u64) {
        if let Some(e) = self.entries.lock().iter_mut().find(|e| e.item.id == id && !e.item.state.finished()) {
            e.cancel.store(true, Ordering::Relaxed);
            e.item.state = State::Canceled;
        }
        changed(app);
    }

    pub fn clear(&self, app: &AppHandle) {
        self.entries.lock().retain(|e| !e.item.state.finished());
        changed(app);
    }
}

fn item(dir: Dir, via: Via, name: String, size: u64, peer: String, state: State) -> Item {
    Item { id: 0, dir, via, name, size, done: 0, peer, state, error: None, uri: None, expires: None }
}

fn cache(app: &AppHandle, sub: &str) -> PathBuf {
    app.path().app_cache_dir().unwrap_or_else(|_| std::env::temp_dir()).join(sub)
}

/* ---------------- 狀態 ---------------- */

pub fn status(app: &AppHandle) -> Status {
    let cloud = app.state::<Cloud>();
    let files = app.state::<Files>();
    if cloud.signed_in() && files.refreshed.lock().is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
        files.wake.notify_one();
    }
    let items: Vec<Item> = files.entries.lock().iter().rev().map(|e| e.item.clone()).collect();
    let peers = if cloud.signed_in() { peers(&cloud, &files) } else { Vec::new() };
    Status { peers, items, inbox: inbox_out(app), remote_mb: cloud.prefs().remote_mb }
}

fn peers(cloud: &Cloud, files: &Files) -> Vec<Peer> {
    let mut out: Vec<Peer> = if cloud.prefs().lan {
        cloud
            .lan
            .nearby(CAP_FILES)
            .into_iter()
            .map(|n| Peer { id: n.id, name: n.name, platform: String::new(), lan: true, ble: false, remote: false })
            .collect()
    } else {
        Vec::new()
    };
    for d in files.devices.lock().iter().filter(|d| d.caps & CAP_DRIVE != 0) {
        match out.iter_mut().find(|p| p.id == d.id) {
            Some(p) => {
                p.remote = true;
                p.platform = d.platform.clone();
            }
            None => out.push(Peer {
                id: d.id.clone(),
                name: d.name.clone(),
                platform: d.platform.clone(),
                lan: false,
                ble: false,
                remote: true,
            }),
        }
    }
    if cloud.prefs().ble {
        let ble = cloud.app().state::<crate::ble::Ble>();
        for (id, name) in ble.peers() {
            match out.iter_mut().find(|p| p.id == id) {
                Some(p) => p.ble = true,
                None => out.push(Peer { id, name, platform: String::new(), lan: false, ble: true, remote: false }),
            }
        }
    }
    out.sort_by(|a, b| b.lan.cmp(&a.lan).then(a.name.cmp(&b.name)));
    out
}

/* ---------------- 傳送 ---------------- */

/// 傳送 Kotlin 準備好的檔案（App 暫存資料夾裡的路徑），回傳排進去的數量。
pub fn send(app: &AppHandle, paths: Vec<String>, peer: String) -> usize {
    let cloud = app.state::<Cloud>();
    let files = app.state::<Files>();
    let target = peers(&cloud, &files).into_iter().find(|p| p.id == peer);
    let Some(target) = target else { return 0 };
    let batch = format!("{:016x}", rand::random::<u64>());
    let list: Vec<(PathBuf, String, u64)> = paths
        .into_iter()
        .map(PathBuf::from)
        .filter_map(|p| {
            let size = std::fs::metadata(&p).ok().filter(|m| m.is_file())?.len();
            let name = p.file_name()?.to_string_lossy().into_owned();
            Some((p, name, size))
        })
        .collect();
    let count = list.len() as u32;
    let jobs: Vec<_> = list
        .into_iter()
        .enumerate()
        .map(|(i, (path, name, size))| {
            let mut offer = Offer::new(
                fp::transfer_id(&cloud.device_id, &path, size, now_ms()),
                name.clone(),
                size,
                cloud.device_name.clone(),
                cloud.device_id.clone(),
            );
            offer.batch = batch.clone();
            offer.index = i as u32;
            offer.count = count;
            // 同一個網路最快；不在同一個網路時，小檔案走藍牙，大檔案走 Google Drive
            let via = if target.lan {
                Via::Lan
            } else if target.ble && crate::ble::small_enough(size) {
                Via::Ble
            } else {
                Via::Drive
            };
            let (id, cancel) = files.add(item(Dir::Out, via, name, size, target.name.clone(), State::Waiting), None);
            (id, cancel, offer, path, via)
        })
        .collect();
    changed(app);
    let app = app.clone();
    let n = jobs.len();
    tauri::async_runtime::spawn(async move {
        for (id, cancel, offer, path, via) in jobs {
            if cancel.load(Ordering::Relaxed) {
                continue;
            }
            match via {
                Via::Lan => send_lan(&app, id, &cancel, offer, &path, &target.id, target.remote).await,
                Via::Ble => send_ble(&app, id, &cancel, &path, &offer.name, offer.size, &target.id, target.remote).await,
                Via::Drive => send_drive(&app, id, &cancel, &path, &offer.name, offer.size, &target.id).await,
            }
            // 傳完（或失敗）就刪掉暫存的複本
            let _ = std::fs::remove_file(&path);
        }
    });
    n
}

async fn cancelled(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn send_lan(app: &AppHandle, id: u64, cancel: &AtomicBool, mut offer: Offer, path: &Path, peer: &str, fallback: bool) {
    let cloud = app.state::<Cloud>();
    let files = app.state::<Files>();
    let mut attempt = 0usize;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let target = cloud.lan.nearby(CAP_FILES).into_iter().find(|n| n.id == peer);
        let key = cloud.key.lock().clone();
        let result = match (target, key) {
            (_, None) => Err(SendError::Rejected("signedOut".into())),
            (None, _) => Err(SendError::Io(io::ErrorKind::NotConnected.into())),
            (Some(target), Some(key)) => {
                files.set(app, id, |it| {
                    it.state = State::Active;
                    it.via = Via::Lan;
                });
                offer.at = now_ms();
                let run = async {
                    let mut stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target.addr))
                        .await
                        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
                    stream.set_nodelay(true)?;
                    stream.write_all(fp::MAGIC).await?;
                    fp::send(&mut stream, &key, &offer, path, |n| files.progress(app, id, n)).await
                };
                tokio::select! {
                    r = run => r,
                    _ = cancelled(cancel) => return,
                }
            }
        };
        match result {
            Ok(()) => {
                return files.set(app, id, |it| {
                    it.state = State::Done;
                    it.done = it.size;
                })
            }
            Err(SendError::Rejected(r)) => return files.fail(app, id, if r == "disk" { "disk" } else { "rejected" }),
            Err(_) => {
                let gone = !cloud.lan.nearby(CAP_FILES).iter().any(|n| n.id == peer);
                if fallback && (gone || attempt >= RETRY_AFTER.len()) && within_limit(&cloud, offer.size) {
                    return send_drive(app, id, cancel, path, &offer.name, offer.size, peer).await;
                }
                let Some(wait) = RETRY_AFTER.get(attempt) else { return files.fail(app, id, "offline") };
                attempt += 1;
                files.set(app, id, |it| it.state = State::Retrying);
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(*wait)) => {}
                    _ = cancelled(cancel) => return,
                }
            }
        }
    }
}

fn within_limit(cloud: &Cloud, size: u64) -> bool {
    size <= cloud.prefs().remote_mb.saturating_mul(1024 * 1024)
}

fn code(e: XferError) -> &'static str {
    match e {
        XferError::Network => "offline",
        XferError::SignedOut => "signedOut",
        XferError::Quota => "quota",
        XferError::Expired => "expired",
        XferError::Corrupt => "corrupt",
        XferError::Disk => "disk",
        XferError::Canceled => "canceled",
    }
}

/// 從藍牙傳一個小檔案；藍牙斷了而且對方在 Drive 的裝置清單裡，就改走 Google Drive。
#[allow(clippy::too_many_arguments)]
async fn send_ble(app: &AppHandle, id: u64, cancel: &AtomicBool, path: &Path, name: &str, size: u64, peer: &str, remote: bool) {
    let files = app.state::<Files>();
    files.set(app, id, |it| it.state = State::Active);
    match crate::ble::send_file(app, peer, path, name).await {
        Ok(()) => files.set(app, id, |it| {
            it.state = State::Done;
            it.done = it.size;
        }),
        Err(_) if remote => {
            files.set(app, id, |it| {
                it.via = Via::Drive;
                it.done = 0;
            });
            send_drive(app, id, cancel, path, name, size, peer).await;
        }
        Err(code) => files.fail(app, id, code),
    }
}

/// 藍牙連著的電腦變了：更新畫面上的傳送對象。
pub fn ble_changed(app: &AppHandle) {
    changed(app);
    app.state::<Cloud>().changed();
}

/// 電腦從藍牙傳來一個小檔案：存到「下載/CopyC」（圖片存到「圖片/CopyC」）、列在傳送紀錄、通知。
pub async fn ble_received(app: &AppHandle, name: &str, bytes: &[u8], from: &str) {
    let files = app.state::<Files>();
    let name = Path::new(name).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let size = bytes.len() as u64;
    let (id, _) = files.add(item(Dir::In, Via::Ble, name.clone(), size, from.to_string(), State::Active), None);
    let dir = app.path().app_cache_dir().unwrap_or_else(|_| std::env::temp_dir()).join("ble-in");
    let _ = tokio::fs::create_dir_all(&dir).await;
    let partial = dir.join(format!("{:016x}", rand::random::<u64>()));
    if tokio::fs::write(&partial, bytes).await.is_err() {
        return files.fail(app, id, "disk");
    }
    let uri = android::save_received(&partial, &name);
    let _ = tokio::fs::remove_file(&partial).await;
    match uri {
        Some(uri) => {
            files.set(app, id, |it| {
                it.state = State::Done;
                it.done = it.size;
                it.uri = Some(uri.clone());
            });
            android::notify_file(from, &name, &uri);
        }
        None => files.fail(app, id, "disk"),
    }
}

async fn send_drive(app: &AppHandle, id: u64, cancel: &AtomicBool, path: &Path, name: &str, size: u64, peer: &str) {
    let cloud = app.state::<Cloud>();
    let files = app.state::<Files>();
    if !within_limit(&cloud, size) {
        return files.fail(app, id, "tooBig");
    }
    files.set(app, id, |it| {
        it.state = State::Active;
        it.via = Via::Drive;
    });
    let result: Result<Manifest, XferError> = async {
        let key = cloud.key().await?;
        let xid: String = (0..12).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
        let token = || async { cloud.access_token().await.map_err(XferError::from) };
        let up = dx::upload(&cloud.http, &dx::GOOGLE, &key, token, &xid, path, size, cancel, |n| files.progress(app, id, n)).await?;
        let m = Manifest {
            v: 1,
            id: xid,
            name: name.to_string(),
            size,
            sha: up.sha,
            data: up.data,
            from: cloud.device_name.clone(),
            from_id: cloud.device_id.clone(),
            to: vec![peer.to_string()],
            at: now_ms(),
        };
        let token = cloud.access_token().await?;
        let drive = Drive { http: &cloud.http, token: &token };
        let mid = dx::put_manifest(&drive, &key, &m).await?;
        cloud.relay(wire::wire(&wire::seal_note(&key, &cloud.device_name, FileNote { m: mid, to: m.to.clone() }), &[]));
        Ok(m)
    }
    .await;
    match result {
        Ok(m) => files.set(app, id, |it| {
            it.state = State::Done;
            it.done = it.size;
            it.expires = Some(m.expires());
        }),
        Err(XferError::Canceled) => {}
        Err(e) => files.fail(app, id, code(e)),
    }
}

/* ---------------- 同一個網路：接收 ---------------- */

/// 區域網路收到 `CPF1` 開頭的連線（開頭 4 個位元組已經讀掉）。
pub async fn receive_lan(app: &AppHandle, mut stream: TcpStream) -> io::Result<()> {
    let cloud = app.state::<Cloud>();
    let files = app.state::<Files>();
    let key = cloud.key.lock().clone().ok_or(io::ErrorKind::PermissionDenied)?;
    let (offer, rel) = fp::read_offer(&mut stream, &key).await?;
    let dir = cache(app, "incoming");
    tokio::fs::create_dir_all(&dir).await?;
    let partial = dir.join(format!("{}.part", offer.id));
    let row = format!("{}:{}", offer.from_id, offer.id);
    let existing = files
        .entries
        .lock()
        .iter()
        .find(|e| e.key.as_deref() == Some(&row) && !e.item.state.finished())
        .map(|e| (e.item.id, e.cancel.clone()));
    let (id, cancel) = existing.unwrap_or_else(|| {
        files.add(item(Dir::In, Via::Lan, offer.name.clone(), offer.size, offer.from.clone(), State::Active), Some(row.clone()))
    });
    files.set(app, id, |it| it.state = State::Active);
    let result = tokio::select! {
        r = fp::receive(&mut stream, &key, &offer, &partial, |n| files.progress(app, id, n)) => r,
        _ = cancelled(&cancel) => {
            drop(stream);
            let _ = tokio::fs::remove_file(&partial).await;
            return Ok(());
        }
    };
    if let Err(e) = result {
        files.set(app, id, |it| {
            if !it.state.finished() {
                it.state = State::Retrying;
            }
        });
        return Err(e);
    }
    // 存到「下載/CopyC」（圖片存到「圖片/CopyC」）；資料夾裡的檔案只保留檔名
    let name = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| offer.name.clone());
    let uri = android::save_received(&partial, &name);
    let _ = tokio::fs::remove_file(&partial).await;
    match uri {
        Some(uri) => {
            files.set(app, id, |it| {
                it.state = State::Done;
                it.done = it.size;
                it.uri = Some(uri.clone());
            });
            android::notify_file(&offer.from, &name, &uri);
        }
        None => files.fail(app, id, "disk"),
    }
    Ok(())
}

/* ---------------- Google Drive：裝置清單、收件匣 ---------------- */

fn inbox_path(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| std::env::temp_dir()).join(INBOX_FILE)
}

fn with_inbox<T>(app: &AppHandle, f: impl FnOnce(&mut Vec<InboxEntry>) -> T) -> T {
    let files = app.state::<Files>();
    let mut guard = files.inbox.lock();
    let inbox = guard.get_or_insert_with(|| {
        std::fs::read(inbox_path(app)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    });
    let now = now_ms();
    for e in inbox.iter_mut() {
        if e.state == InboxState::Ready && e.m.expired(now) {
            e.state = InboxState::Expired;
        }
    }
    inbox.retain(|e| now < e.m.expires() + INBOX_KEEP_AFTER);
    let out = f(inbox);
    let _ = std::fs::write(inbox_path(app), serde_json::to_vec(inbox).unwrap_or_default());
    out
}

fn inbox_out(app: &AppHandle) -> Vec<InboxOut> {
    if !app.state::<Cloud>().signed_in() {
        return Vec::new();
    }
    let mut list: Vec<InboxOut> = with_inbox(app, |inbox| {
        inbox
            .iter()
            .map(|e| InboxOut {
                mid: e.mid.clone(),
                name: e.m.name.clone(),
                size: e.m.size,
                from: e.m.from.clone(),
                expires: e.m.expires(),
                state: e.state,
                error: e.error,
                uri: e.uri.clone(),
            })
            .collect()
    });
    list.sort_by(|a, b| b.expires.cmp(&a.expires));
    list
}

/// 登出：清掉收件匣與裝置清單。
pub fn forget(app: &AppHandle) {
    let files = app.state::<Files>();
    *files.inbox.lock() = Some(Vec::new());
    let _ = std::fs::remove_file(inbox_path(app));
    files.devices.lock().clear();
    *files.published.lock() = None;
    *files.devices_at.lock() = None;
}

fn for_me(cloud: &Cloud, m: &Manifest) -> bool {
    m.from_id != cloud.device_id && (m.to.is_empty() || m.to.contains(&cloud.device_id))
}

fn add_inbox(app: &AppHandle, mid: String, m: Manifest) {
    let added = with_inbox(app, |inbox| {
        if inbox.iter().any(|e| e.mid == mid || e.m.id == m.id) {
            return false;
        }
        inbox.push(InboxEntry { mid: mid.clone(), m: m.clone(), state: InboxState::Ready, uri: None, error: None });
        true
    });
    if added {
        changed(app);
        android::notify_incoming(&m.from, &m.name);
    }
}

/// 即時同步收到「有檔案在 Drive 等你」。
pub fn on_note(app: &AppHandle, note: FileNote) {
    let device = app.state::<Cloud>().device_id.clone();
    if !note.to.is_empty() && !note.to.contains(&device) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let cloud = app.state::<Cloud>();
        let (Ok(key), Ok(token)) = (cloud.key().await, cloud.access_token().await) else { return };
        let drive = Drive { http: &cloud.http, token: &token };
        if let Some(m) = dx::get_manifest(&drive, &key, &note.m).await {
            if for_me(&cloud, &m) && !m.expired(now_ms()) {
                add_inbox(&app, note.m, m);
            }
        }
    });
}

/// 背景工作：登入時定期更新裝置資料、檢查收件匣、刪掉滿 24 小時的檔案。
pub async fn run(app: AppHandle) {
    loop {
        let files = app.state::<Files>();
        if app.state::<Cloud>().signed_in() {
            let _ = once(&app).await;
        }
        tokio::select! {
            _ = tokio::time::sleep(REFRESH) => {}
            _ = files.wake.notified() => {}
        }
    }
}

async fn once(app: &AppHandle) -> Result<(), XferError> {
    let cloud = app.state::<Cloud>();
    let files = app.state::<Files>();
    *files.refreshed.lock() = Some(Instant::now());
    let key = cloud.key().await?;
    let token = cloud.access_token().await?;
    let drive = Drive { http: &cloud.http, token: &token };
    let list = drive.list().await?;
    let now = now_ms();
    if files.published.lock().is_none_or(|t| t.elapsed() > PUBLISH_EVERY) {
        let me = DeviceInfo {
            v: 1,
            id: cloud.device_id.clone(),
            name: cloud.device_name.clone(),
            platform: "android".into(),
            caps: CAP_FILES | CAP_DRIVE,
            seen: now,
        };
        if dx::publish_device(&drive, &list, &key, &me).await.is_ok() {
            *files.published.lock() = Some(Instant::now());
        }
    }
    if files.devices_at.lock().is_none_or(|t| t.elapsed() > DEVICES_EVERY) {
        *files.devices.lock() = dx::read_devices(&drive, &list, &key, &cloud.device_id, now).await;
        *files.devices_at.lock() = Some(Instant::now());
    }
    let known: HashSet<String> = with_inbox(app, |inbox| inbox.iter().map(|e| e.mid.clone()).collect());
    for f in list.iter().filter(|f| f.name.starts_with("x1-") && f.name.ends_with(".m")) {
        if known.contains(&f.id) || files.skipped.lock().contains(&f.id) {
            continue;
        }
        match dx::get_manifest(&drive, &key, &f.id).await {
            Some(m) if for_me(&cloud, &m) && !m.expired(now) => add_inbox(app, f.id.clone(), m),
            _ => {
                files.skipped.lock().insert(f.id.clone());
            }
        }
    }
    for id in dx::expired_files(&list, now) {
        let _ = drive.delete(&id).await;
    }
    changed(app);
    Ok(())
}

/// 下載收件匣裡的檔案；超過 24 小時回傳 "expired"。
pub async fn download(app: &AppHandle, mid: &str) -> Result<(), String> {
    let cloud = app.state::<Cloud>();
    let files = app.state::<Files>();
    let entry = with_inbox(app, |inbox| inbox.iter().find(|e| e.mid == mid).cloned()).ok_or("expired")?;
    if entry.m.expired(now_ms()) {
        set_entry(app, mid, |e| e.state = InboxState::Expired);
        changed(app);
        return Err("expired".into());
    }
    if entry.state == InboxState::Downloading {
        return Ok(());
    }
    let m = entry.m;
    let dir = cache(app, "incoming");
    tokio::fs::create_dir_all(&dir).await.map_err(|_| "disk")?;
    let partial = dir.join(format!("{}.dpart", m.id));
    let (id, cancel) = files.add(item(Dir::In, Via::Drive, m.name.clone(), m.size, m.from.clone(), State::Active), None);
    set_entry(app, mid, |e| {
        e.state = InboxState::Downloading;
        e.error = None;
    });
    changed(app);
    let result = async {
        let key = cloud.key().await?;
        let token = || async { cloud.access_token().await.map_err(XferError::from) };
        dx::download(&cloud.http, &dx::GOOGLE, &key, token, &m, &partial, &cancel, |n| files.progress(app, id, n)).await
    }
    .await;
    match result {
        Ok(()) => {
            let name = fp::safe_relative(&m.name)
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "CopyC file".into());
            let uri = android::save_received(&partial, &name);
            let _ = tokio::fs::remove_file(&partial).await;
            let Some(uri) = uri else {
                files.fail(app, id, "disk");
                set_entry(app, mid, |e| {
                    e.state = InboxState::Ready;
                    e.error = Some(XferError::Disk);
                });
                return Err("disk".into());
            };
            files.set(app, id, |it| {
                it.state = State::Done;
                it.done = it.size;
                it.uri = Some(uri.clone());
            });
            set_entry(app, mid, |e| {
                e.state = InboxState::Done;
                e.uri = Some(uri);
            });
            changed(app);
            Ok(())
        }
        Err(e) => {
            if e == XferError::Canceled {
                files.set(app, id, |it| it.state = State::Canceled);
            } else {
                files.fail(app, id, code(e));
            }
            set_entry(app, mid, |en| {
                en.state = if e == XferError::Expired { InboxState::Expired } else { InboxState::Ready };
                en.error = (e != XferError::Canceled).then_some(e);
            });
            changed(app);
            Err(code(e).into())
        }
    }
}

fn set_entry(app: &AppHandle, mid: &str, f: impl FnOnce(&mut InboxEntry)) {
    with_inbox(app, |inbox| {
        if let Some(e) = inbox.iter_mut().find(|e| e.mid == mid) {
            f(e);
        }
    });
}
