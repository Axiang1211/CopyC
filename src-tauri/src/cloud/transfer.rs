//! 檔案互傳（M1）：同一個網路裡、登入同一個 Google 帳號的電腦之間，直接傳檔案、資料夾與圖片。
//!
//! - 傳送：拖進偏好設定、按「選擇檔案」或「傳送剪貼簿」。同一批檔案依序傳，斷線會自動重試並從中斷處接著傳。
//! - 接收：同一個帳號的裝置傳來就直接收下（只有拿得到帳號金鑰的裝置才連得進來），存到「下載/CopyC」。
//!   一批收完後放進剪貼簿：單張圖片放圖片本身，其他放檔案，馬上就能貼上。
//!
//! 協定在 [`super::file_proto`]；探索沿用區域網路直傳的信標（[`super::lan`]）。
//! 對方不在同一個網路時改經由 Google Drive 傳（M3，見 [`super::remote_xfer`]）。

use super::{
    file_proto::{self as fp, Offer, SendError},
    lan_proto::{Nearby, CAP_DRIVE, CAP_FILES},
    remote_xfer, Cloud,
};
use crate::state::{now_ms, AppState};
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::{io::AsyncWriteExt, net::TcpStream};

/// 接收中的暫存檔放在接收資料夾裡的這個子資料夾（同一顆磁碟，收完直接改名）。
pub(super) const PARTIAL_DIR: &str = ".copyc-partial";
/// 這麼久沒有接續的暫存檔就刪掉。
const PARTIAL_KEEP: Duration = Duration::from_secs(7 * 24 * 3600);
/// 斷線後依序等這麼多秒再試（總共約 4 分鐘），之後才算失敗。
const RETRY_AFTER: [u64; 8] = [2, 4, 8, 15, 30, 60, 60, 60];
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
/// 進度事件最多每 0.25 秒送一次給偏好設定。
const EMIT_EVERY: Duration = Duration::from_millis(250);
/// 清單最多留這麼多筆。
const KEEP_ITEMS: usize = 200;
/// 接收端這麼久沒等到傳送端重新連上，就把這筆標成失敗。
const IN_STALE: Duration = Duration::from_secs(5 * 60);
/// 收到的單張圖片小於這個大小才放進剪貼簿當圖片（再大就放檔案）。
const CLIPBOARD_IMAGE_MAX: u64 = 40 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Out,
    In,
}

/// 經由哪裡傳：同一個網路直接傳，或經由 Google Drive。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Via {
    Lan,
    Drive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferState {
    Waiting,
    Active,
    /// 斷線了，等對方回來後接著傳
    Retrying,
    Done,
    Failed,
    Canceled,
}

impl TransferState {
    fn finished(self) -> bool {
        matches!(self, TransferState::Done | TransferState::Failed | TransferState::Canceled)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: u64,
    dir: Direction,
    pub(super) via: Via,
    name: String,
    pub(super) size: u64,
    pub(super) done: u64,
    /// 對方裝置的名稱
    peer: String,
    pub(super) state: TransferState,
    /// 失敗原因的代碼，前端翻譯（offline / rejected / disk / missing / signedOut / quota / tooBig / expired / corrupt）
    pub(super) error: Option<String>,
    /// 傳送：本機原檔；接收：存好的位置
    pub(super) path: Option<String>,
    /// 每秒位元組數
    speed: f64,
    at: u64,
    /// 經由 Drive 傳出去的檔案：對方可以下載到什麼時候
    pub(super) expires: Option<u64>,
}

impl Item {
    pub(super) fn new(dir: Direction, via: Via, name: String, size: u64, peer: String, state: TransferState) -> Self {
        Item {
            id: 0,
            dir,
            via,
            name,
            size,
            done: 0,
            peer,
            state,
            error: None,
            path: None,
            speed: 0.0,
            at: now_ms(),
            expires: None,
        }
    }
}

struct Entry {
    item: Item,
    /// 接收時對應的傳送編號（同一個檔案斷線重連時沿用同一筆）
    key: Option<String>,
    cancel: Arc<AtomicBool>,
    emitted: Instant,
    sample: (Instant, u64),
    touched: Instant,
}

struct Batch {
    count: u32,
    seen: u32,
    tops: Vec<PathBuf>,
    device: String,
    started: Instant,
}

#[derive(Default)]
pub struct Transfers {
    entries: Mutex<Vec<Entry>>,
    next: AtomicU64,
    batches: Mutex<HashMap<String, Batch>>,
    pub(super) remote: remote_xfer::Remote,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Ready {
    Ok,
    SignedOut,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub id: String,
    pub name: String,
    /// windows / macos / android（只在 Drive 的裝置清單裡有）
    platform: String,
    /// 在同一個網路裡（直接傳）
    lan: bool,
    /// 可以經由 Drive 傳
    remote: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    ready: Ready,
    /// 即時同步或「同一個網路直接互傳」沒有開：只能經由 Drive 傳
    lan_off: bool,
    peers: Vec<Peer>,
    items: Vec<Item>,
    inbox: Vec<remote_xfer::InboxOut>,
    dir: String,
    remote_mb: u64,
}

impl Transfers {
    pub(super) fn add(&self, mut item: Item, key: Option<String>) -> (u64, Arc<AtomicBool>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        item.id = id;
        let cancel = Arc::new(AtomicBool::new(false));
        let now = Instant::now();
        let mut entries = self.entries.lock();
        entries.push(Entry { item, key, cancel: cancel.clone(), emitted: now, sample: (now, 0), touched: now });
        // 只留最近的紀錄；進行中的不會被清掉
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

    /// 同一個檔案的接收（斷線重連時沿用同一筆）。
    fn find_in(&self, key: &str) -> Option<(u64, Arc<AtomicBool>)> {
        self.entries
            .lock()
            .iter()
            .find(|e| e.key.as_deref() == Some(key) && !e.item.state.finished())
            .map(|e| (e.item.id, e.cancel.clone()))
    }

    pub(super) fn fail(&self, app: &AppHandle, id: u64, code: &str) {
        self.set(app, id, |it| {
            it.state = TransferState::Failed;
            it.error = Some(code.to_string());
        });
    }

    pub(super) fn set(&self, app: &AppHandle, id: u64, f: impl FnOnce(&mut Item)) {
        if let Some(e) = self.entries.lock().iter_mut().find(|e| e.item.id == id) {
            f(&mut e.item);
            e.touched = Instant::now();
            if e.item.state != TransferState::Active {
                e.item.speed = 0.0;
            }
            e.emitted = Instant::now();
        }
        let _ = app.emit("transfer:changed", ());
    }

    pub(super) fn progress(&self, app: &AppHandle, id: u64, done: u64) {
        let mut emit = false;
        if let Some(e) = self.entries.lock().iter_mut().find(|e| e.item.id == id) {
            e.item.done = done;
            e.touched = Instant::now();
            let (t0, d0) = e.sample;
            let dt = t0.elapsed().as_secs_f64();
            if done < d0 {
                e.sample = (Instant::now(), done);
            } else if dt >= 0.5 {
                let rate = (done - d0) as f64 / dt;
                e.item.speed = if e.item.speed == 0.0 { rate } else { e.item.speed * 0.6 + rate * 0.4 };
                e.sample = (Instant::now(), done);
            }
            if e.emitted.elapsed() >= EMIT_EVERY {
                e.emitted = Instant::now();
                emit = true;
            }
        }
        if emit {
            let _ = app.emit("transfer:changed", ());
        }
    }

    fn items(&self) -> Vec<Item> {
        let mut entries = self.entries.lock();
        for e in entries.iter_mut() {
            // 接收端一直等不到傳送端重新連上
            if e.item.dir == Direction::In && e.item.state == TransferState::Retrying && e.touched.elapsed() > IN_STALE {
                e.item.state = TransferState::Failed;
                e.item.error = Some("offline".into());
            }
        }
        entries.iter().rev().map(|e| e.item.clone()).collect()
    }

    fn get(&self, id: u64) -> Option<Item> {
        self.entries.lock().iter().find(|e| e.item.id == id).map(|e| e.item.clone())
    }

    pub fn cancel(&self, app: &AppHandle, id: u64) {
        let mut changed = false;
        if let Some(e) = self.entries.lock().iter_mut().find(|e| e.item.id == id && !e.item.state.finished()) {
            e.cancel.store(true, Ordering::Relaxed);
            e.item.state = TransferState::Canceled;
            e.item.speed = 0.0;
            changed = true;
        }
        if changed {
            let _ = app.emit("transfer:changed", ());
        }
    }

    pub fn clear_finished(&self, app: &AppHandle) {
        self.entries.lock().retain(|e| !e.item.state.finished());
        let _ = app.emit("transfer:changed", ());
    }
}

/* ---------------- 狀態與設定 ---------------- */

pub fn status(app: &AppHandle) -> Status {
    let cloud = app.state::<Cloud>();
    let c = *cloud.config.borrow();
    let ready = if c.signed_in { Ready::Ok } else { Ready::SignedOut };
    let peers = if c.signed_in { peers(&cloud) } else { Vec::new() };
    if c.signed_in {
        cloud.transfers.remote.poke();
    }
    Status {
        ready,
        lan_off: !(c.live && c.lan),
        peers,
        items: cloud.transfers.items(),
        inbox: if c.signed_in { remote_xfer::inbox(&cloud) } else { Vec::new() },
        dir: dest_dir(app).to_string_lossy().into_owned(),
        remote_mb: app.state::<AppState>().settings.lock().transfer_remote_mb,
    }
}

/// 有沒有其他裝置可以傳（通知框要不要顯示「發送到…」）。
pub fn has_peers(app: &AppHandle) -> bool {
    let cloud = app.state::<Cloud>();
    let signed_in = cloud.config.borrow().signed_in;
    signed_in && !peers(&cloud).is_empty()
}

/// 通知框「發送到…」列出的裝置。
pub fn peer_list(app: &AppHandle) -> Vec<Peer> {
    let cloud = app.state::<Cloud>();
    let signed_in = cloud.config.borrow().signed_in;
    if signed_in {
        peers(&cloud)
    } else {
        Vec::new()
    }
}

/// 可以傳給誰：同一個網路裡的裝置，加上 Drive 裝置清單裡的其他裝置。
fn peers(cloud: &Cloud) -> Vec<Peer> {
    let mut out: Vec<Peer> = cloud
        .lan
        .nearby(CAP_FILES)
        .into_iter()
        .map(|n| Peer { id: n.id, name: n.name, platform: String::new(), lan: true, remote: false })
        .collect();
    for d in cloud.transfers.remote.devices() {
        if d.caps & CAP_DRIVE == 0 {
            continue;
        }
        match out.iter_mut().find(|p| p.id == d.id) {
            Some(p) => {
                p.remote = true;
                p.platform = d.platform;
                if p.name.is_empty() {
                    p.name = d.name;
                }
            }
            None => out.push(Peer { id: d.id, name: d.name, platform: d.platform, lan: false, remote: true }),
        }
    }
    // 同一個網路裡的排前面
    out.sort_by(|a, b| b.lan.cmp(&a.lan).then(a.name.cmp(&b.name)));
    out
}

/// 收到的檔案放在哪裡：使用者選的資料夾，預設是「下載/CopyC」。
pub fn dest_dir(app: &AppHandle) -> PathBuf {
    let chosen = app.state::<AppState>().settings.lock().transfer_dir.clone();
    if !chosen.is_empty() {
        return PathBuf::from(chosen);
    }
    app.path()
        .download_dir()
        .or_else(|_| app.path().home_dir().map(|h| h.join("Downloads")))
        .or_else(|_| app.path().app_data_dir())
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("CopyC")
}

/* ---------------- 傳送 ---------------- */

/// 一批要傳的檔案：(本機路徑, 傳給對方的名稱)。資料夾會展開成裡面的所有檔案。
fn expand(paths: &[PathBuf]) -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(PathBuf, String)>) {
        let Ok(read) = std::fs::read_dir(dir) else { return };
        let mut children: Vec<_> = read.flatten().collect();
        children.sort_by_key(|e| e.file_name());
        for child in children {
            let Ok(kind) = child.file_type() else { continue };
            let name = format!("{prefix}/{}", child.file_name().to_string_lossy());
            if kind.is_dir() {
                walk(&child.path(), &name, out);
            } else if kind.is_file() {
                out.push((child.path(), name));
            }
        }
    }
    let mut out = Vec::new();
    for path in paths {
        let Ok(meta) = std::fs::symlink_metadata(path) else { continue };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        if meta.is_dir() {
            walk(path, &name, &mut out);
        } else if meta.is_file() || std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
            out.push((path.clone(), name));
        }
    }
    out
}

/// 把檔案或資料夾傳給 `peer`，回傳排進去的檔案數。
pub fn send_paths(app: &AppHandle, paths: Vec<PathBuf>, peer: String) -> usize {
    let files = expand(&paths);
    if files.is_empty() {
        return 0;
    }
    let cloud = app.state::<Cloud>();
    let target = peers(&cloud).into_iter().find(|p| p.id == peer);
    let peer_name = target.as_ref().map(|p| p.name.clone()).unwrap_or_default();
    let (lan, remote) = target.as_ref().map_or((false, false), |p| (p.lan, p.remote));
    let batch = format!("{:016x}", rand::random::<u64>());
    let count = files.len() as u32;
    let jobs: Vec<_> = files
        .into_iter()
        .enumerate()
        .map(|(i, (path, name))| {
            let meta = std::fs::metadata(&path).ok();
            let size = meta.as_ref().map_or(0, |m| m.len());
            let modified = meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64);
            let mut offer = Offer::new(
                fp::transfer_id(&cloud.device_id, &path, size, modified),
                name.clone(),
                size,
                cloud.device_name.clone(),
                cloud.device_id.clone(),
            );
            offer.batch = batch.clone();
            offer.index = i as u32;
            offer.count = count;
            let via = if lan { Via::Lan } else { Via::Drive };
            let mut item = Item::new(Direction::Out, via, name, size, peer_name.clone(), TransferState::Waiting);
            item.path = Some(path.to_string_lossy().into_owned());
            let (id, cancel) = cloud.transfers.add(item, None);
            (id, cancel, offer, path)
        })
        .collect();
    let _ = app.emit("transfer:changed", ());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        for (id, cancel, offer, path) in jobs {
            if cancel.load(Ordering::Relaxed) {
                continue;
            }
            if lan {
                send_one(&app, id, &cancel, offer, &path, &peer, remote).await;
            } else if remote {
                remote_xfer::send_one(&app, id, &cancel, &path, &offer.name, offer.size, &peer).await;
            } else {
                app.state::<Cloud>().transfers.fail(&app, id, "offline");
            }
        }
    });
    count as usize
}

async fn cancelled(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 同一個網路直接傳。`fallback`：一直連不上時改經由 Drive 傳（大小在上限內才會）。
async fn send_one(app: &AppHandle, id: u64, cancel: &AtomicBool, mut offer: Offer, path: &Path, peer: &str, fallback: bool) {
    let cloud = app.state::<Cloud>();
    let transfers = &cloud.transfers;
    let mut attempt = 0usize;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let target: Option<Nearby> = cloud.lan.nearby(CAP_FILES).into_iter().find(|n| n.id == peer);
        let key = cloud.key.lock().clone();
        let outcome = match (target, key) {
            (_, None) => Err(SendError::Rejected("signedOut".into())),
            (None, _) => Err(SendError::Io(io::ErrorKind::NotConnected.into())),
            (Some(target), Some(key)) => {
                transfers.set(app, id, |it| {
                    it.state = TransferState::Active;
                    it.error = None;
                    if !target.name.is_empty() {
                        it.peer = target.name.clone();
                    }
                });
                offer.at = now_ms();
                let run = async {
                    let mut stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target.addr))
                        .await
                        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
                    stream.set_nodelay(true)?;
                    stream.write_all(fp::MAGIC).await?;
                    fp::send(&mut stream, &key, &offer, path, |n| transfers.progress(app, id, n)).await
                };
                tokio::select! {
                    r = run => r,
                    _ = cancelled(cancel) => return,
                }
            }
        };
        match outcome {
            Ok(()) => {
                transfers.set(app, id, |it| {
                    it.state = TransferState::Done;
                    it.done = it.size;
                });
                return;
            }
            Err(SendError::Rejected(reason)) => {
                let code = match reason.as_str() {
                    "disk" | "signedOut" => reason,
                    _ => "rejected".into(),
                };
                transfers.set(app, id, |it| {
                    it.state = TransferState::Failed;
                    it.error = Some(code);
                });
                return;
            }
            Err(SendError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {
                // 原檔被移走或刪掉了
                transfers.set(app, id, |it| {
                    it.state = TransferState::Failed;
                    it.error = Some("missing".into());
                });
                return;
            }
            Err(SendError::Io(_)) | Err(SendError::Corrupt) => {
                // 對方離開了這個網路（不在清單裡），或重試完了：可以的話改經由 Drive 傳
                let gone = !cloud.lan.nearby(CAP_FILES).iter().any(|n| n.id == peer);
                if fallback && (gone || attempt >= RETRY_AFTER.len()) && remote_xfer::within_limit(app, offer.size) {
                    return remote_xfer::send_one(app, id, cancel, path, &offer.name, offer.size, peer).await;
                }
                let Some(wait) = RETRY_AFTER.get(attempt) else {
                    transfers.set(app, id, |it| {
                        it.state = TransferState::Failed;
                        it.error = Some("offline".into());
                    });
                    return;
                };
                attempt += 1;
                transfers.set(app, id, |it| it.state = TransferState::Retrying);
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(*wait)) => {}
                    _ = cancelled(cancel) => return,
                }
            }
        }
    }
}

/// 剪貼簿裡有檔案就傳檔案，有圖片就存成 PNG 再傳。回傳排進去的檔案數（0 表示剪貼簿裡沒有可以傳的東西）。
pub fn send_clipboard(app: &AppHandle, peer: String) -> usize {
    let Ok(mut cb) = arboard::Clipboard::new() else { return 0 };
    if let Ok(files) = cb.get().file_list() {
        if !files.is_empty() {
            return send_paths(app, files, peer);
        }
    }
    let Ok(img) = cb.get_image() else { return 0 };
    let Some(rgba) = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned()) else {
        return 0;
    };
    let dir = outbox(app);
    let _ = std::fs::create_dir_all(&dir);
    let stamp = chrono_like(now_ms());
    let path = dir.join(format!("CopyC {stamp}.png"));
    if rgba.save_with_format(&path, image::ImageFormat::Png).is_err() {
        return 0;
    }
    send_paths(app, vec![path], peer)
}

fn outbox(app: &AppHandle) -> PathBuf {
    app.path().app_cache_dir().unwrap_or_else(|_| std::env::temp_dir()).join("outbox")
}

/// 「2026-11-17 13.05.22」形式的本機時間（檔名用，不含冒號）。
fn chrono_like(ms: u64) -> String {
    let offset = local_offset_secs();
    let secs = (ms / 1000) as i64 + offset;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // 1970-01-01 起算的日數換成年月日（Howard Hinnant 的演算法）
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}.{:02}.{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// 本機時區與 UTC 的差（秒）。取不到就用 UTC。
fn local_offset_secs() -> i64 {
    #[cfg(windows)]
    {
        use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
        let mut tz = TIME_ZONE_INFORMATION::default();
        let r = unsafe { GetTimeZoneInformation(&mut tz) };
        let bias = tz.Bias + if r == 2 { tz.DaylightBias } else { 0 };
        -(bias as i64) * 60
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                let s = s.trim();
                let sign = if s.starts_with('-') { -1 } else { 1 };
                let digits = s.trim_start_matches(['+', '-']);
                let h: i64 = digits.get(0..2)?.parse().ok()?;
                let m: i64 = digits.get(2..4)?.parse().ok()?;
                Some(sign * (h * 3600 + m * 60))
            })
            .unwrap_or(0)
    }
}

/* ---------------- 接收 ---------------- */

/// 收一個檔案（區域網路監聽收到 `CPF1` 開頭的連線時呼叫，開頭 4 個位元組已經讀掉）。
pub async fn receive(app: &AppHandle, mut stream: TcpStream) -> io::Result<()> {
    let cloud = app.state::<Cloud>();
    let key = cloud.key.lock().clone().ok_or(io::ErrorKind::PermissionDenied)?;
    let (offer, rel) = fp::read_offer(&mut stream, &key).await?;
    if offer.from_id == cloud.device_id {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let dest = dest_dir(app);
    let partial_dir = dest.join(PARTIAL_DIR);
    if tokio::fs::create_dir_all(&partial_dir).await.is_err() {
        return fp::reject(&mut stream, &key, "disk").await;
    }
    hide(&partial_dir);
    let partial = partial_dir.join(format!("{}.part", offer.id));
    let transfers = &cloud.transfers;
    let row = format!("{}:{}", offer.from_id, offer.id);
    let (id, cancel) = transfers.find_in(&row).unwrap_or_else(|| {
        let item = Item::new(Direction::In, Via::Lan, offer.name.clone(), offer.size, offer.from.clone(), TransferState::Active);
        transfers.add(item, Some(row.clone()))
    });
    transfers.set(app, id, |it| it.state = TransferState::Active);

    let result = tokio::select! {
        r = fp::receive(&mut stream, &key, &offer, &partial, |n| transfers.progress(app, id, n)) => r,
        _ = cancelled(&cancel) => {
            drop(stream);
            let _ = tokio::fs::remove_file(&partial).await;
            return Ok(());
        }
    };
    if let Err(e) = result {
        let disk = matches!(e.kind(), io::ErrorKind::StorageFull | io::ErrorKind::PermissionDenied);
        transfers.set(app, id, |it| {
            if disk {
                it.state = TransferState::Failed;
                it.error = Some("disk".into());
            } else if !it.state.finished() {
                it.state = TransferState::Retrying;
            }
        });
        return Err(e);
    }

    let final_path = fp::unique_path(&dest, &rel);
    if let Some(parent) = final_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    if tokio::fs::rename(&partial, &final_path).await.is_err() {
        tokio::fs::copy(&partial, &final_path).await?;
        let _ = tokio::fs::remove_file(&partial).await;
    }
    transfers.set(app, id, |it| {
        it.state = TransferState::Done;
        it.done = it.size;
        it.path = Some(final_path.to_string_lossy().into_owned());
    });
    finish_batch(app, &offer, &dest, &rel, &final_path);
    Ok(())
}

/// 記下這批收到的檔案；整批收完就放進剪貼簿。
fn finish_batch(app: &AppHandle, offer: &Offer, dest: &Path, rel: &Path, saved: &Path) {
    let batch_id = if offer.batch.is_empty() { offer.id.clone() } else { offer.batch.clone() };
    let nested = rel.components().count() > 1;
    let top = if nested { rel.components().next().map(|c| dest.join(c)) } else { Some(saved.to_path_buf()) };
    let cloud = app.state::<Cloud>();
    let ready = {
        let mut batches = cloud.transfers.batches.lock();
        batches.retain(|_, b| b.started.elapsed() < Duration::from_secs(3600));
        let b = batches.entry(batch_id.clone()).or_insert_with(|| Batch {
            count: offer.count.max(1),
            seen: 0,
            tops: Vec::new(),
            device: offer.from.clone(),
            started: Instant::now(),
        });
        b.seen += 1;
        if let Some(top) = top {
            if !b.tops.contains(&top) {
                b.tops.push(top);
            }
        }
        if b.seen >= b.count {
            batches.remove(&batch_id)
        } else {
            None
        }
    };
    let Some(batch) = ready else { return };
    put_on_clipboard(app, batch.tops, &batch.device);
}

/// 把收到的檔案放進剪貼簿：單張圖片放圖片本身，其他放檔案。通知框會標示是哪台裝置傳來的。
pub(super) fn put_on_clipboard(app: &AppHandle, tops: Vec<PathBuf>, device: &str) {
    if !app.state::<AppState>().settings.lock().transfer_clipboard || tops.is_empty() {
        return;
    }
    let image = match tops.as_slice() {
        [one] if is_image(one) && std::fs::metadata(one).is_ok_and(|m| m.len() <= CLIPBOARD_IMAGE_MAX) => Some(one.clone()),
        _ => None,
    };
    app.state::<Cloud>().expect_incoming_any(device.to_string());
    std::thread::spawn(move || {
        let Ok(mut cb) = arboard::Clipboard::new() else { return };
        if let Some(img) = image.and_then(|p| image::open(p).ok()) {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            let data = arboard::ImageData { width: w as usize, height: h as usize, bytes: rgba.into_raw().into() };
            if cb.set_image(data).is_ok() {
                return;
            }
        }
        let _ = cb.set().file_list(&tops);
    });
}

fn is_image(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp")
}

/// Windows：暫存資料夾設成隱藏，不會出現在使用者的「下載」裡。
fn hide(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::{
            core::PCWSTR,
            Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN},
        };
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        unsafe {
            let _ = SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_HIDDEN);
        }
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// 啟動時清掉太久沒有接續的暫存檔，以及傳完的剪貼簿圖片。
pub fn cleanup(app: &AppHandle) {
    let old = |p: &Path, age: Duration| {
        std::fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|e| e > age))
    };
    for (dir, age) in [(dest_dir(app).join(PARTIAL_DIR), PARTIAL_KEEP), (outbox(app), Duration::from_secs(24 * 3600))] {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if old(&entry.path(), age) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/* ---------------- 經由 Drive 傳來的檔案 ---------------- */

/// 收件匣裡這個檔案還能不能下載（超過 24 小時回傳 "expired"）。
pub fn inbox_ready(app: &AppHandle, mid: &str) -> Result<(), &'static str> {
    let cloud = app.state::<Cloud>();
    match remote_xfer::inbox(&cloud).into_iter().find(|e| e.mid() == mid) {
        Some(e) if e.expired_now() => Err("expired"),
        Some(_) => Ok(()),
        None => Err("expired"),
    }
}

pub async fn download(app: &AppHandle, mid: &str) -> Result<(), &'static str> {
    remote_xfer::download(app, mid).await.map_err(remote_xfer::code)
}

pub fn inbox_file(app: &AppHandle, mid: &str) -> Option<PathBuf> {
    remote_xfer::inbox_path_of(app, mid)
}

/* ---------------- 打開檔案 ---------------- */

pub fn item(app: &AppHandle, id: u64) -> Option<Item> {
    app.state::<Cloud>().transfers.get(id)
}

impl Item {
    pub fn local_path(&self) -> Option<PathBuf> {
        self.path.as_ref().map(PathBuf::from).filter(|p| p.exists())
    }
}

/// 用系統預設的程式打開。
pub fn open_path(path: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// 在檔案總管 / Finder 裡顯示並選取。
pub fn reveal(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_expand_to_relative_names() {
        let root = std::env::temp_dir().join(format!("copyc-expand-{}", rand::random::<u32>()));
        std::fs::create_dir_all(root.join("相片").join("2026")).unwrap();
        std::fs::write(root.join("相片").join("a.jpg"), b"1").unwrap();
        std::fs::write(root.join("相片").join("2026").join("b.jpg"), b"2").unwrap();
        std::fs::write(root.join("報告.pdf"), b"3").unwrap();
        let got = expand(&[root.join("相片"), root.join("報告.pdf"), root.join("不存在")]);
        let names: Vec<_> = got.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, ["相片/2026/b.jpg", "相片/a.jpg", "報告.pdf"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_name_timestamps() {
        // 2026-11-17 05:05:22 UTC
        let utc = 1_794_891_922_000i64 - local_offset_secs() * 1000;
        let s = chrono_like(utc as u64);
        assert_eq!(s, "2026-11-17 05.05.22");
    }

    #[test]
    fn image_extensions() {
        assert!(is_image(Path::new("a.JPG")));
        assert!(is_image(Path::new("x/y.webp")));
        assert!(!is_image(Path::new("a.pdf")));
        assert!(!is_image(Path::new("noext")));
    }
}
