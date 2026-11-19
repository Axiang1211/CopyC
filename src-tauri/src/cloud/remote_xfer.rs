//! 跨網路傳檔（M3，電腦版）：對方不在同一個網路時，檔案加密後經由使用者自己的 Google Drive 傳過去。
//!
//! - 傳送：上傳內容與說明檔（[`super::drive_xfer`]），再經由即時同步通知對方。
//! - 接收：收到通知或定期檢查 Drive 時，把傳給這台電腦的檔案列在「收到的檔案」，使用者按「下載」才下載。
//! - 檔案在 Drive 上保存 24 小時，之後由任何一台裝置刪除；超過 24 小時按下載會顯示「已過期」。
//! - 裝置清單：每台裝置在 Drive 留一份加密的名稱與平台，「傳送到」才列得出不在同一個網路的裝置。

use super::{
    drive::Drive,
    drive_xfer::{self as dx, DeviceInfo, Manifest, XferError},
    file_proto,
    lan_proto::{CAP_DRIVE, CAP_FILES},
    transfer::{self, Direction, Item, TransferState, Via},
    wire::{self, FileNote},
    Cloud,
};
use crate::state::{now_ms, AppState};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

/// 「跨網路傳送上限」可選的值（MB）。預設 100 MB；最大 200 GB。
pub const LIMITS_MB: [u64; 7] = [100, 500, 1024, 5120, 20_480, 51_200, 204_800];
/// 沒有通知時多久檢查一次 Drive。
const REFRESH: Duration = Duration::from_secs(10 * 60);
/// 裝置清單多久重新讀一次。
const DEVICES_EVERY: Duration = Duration::from_secs(30 * 60);
/// 這台裝置的資料多久更新一次。
const PUBLISH_EVERY: Duration = Duration::from_secs(6 * 3600);
/// 「收到的檔案」裡過期的項目再留這麼久（讓使用者看得到「已過期」）。
const INBOX_KEEP_AFTER: u64 = 24 * 3600 * 1000;
const INBOX_FILE: &str = "inbox.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InboxState {
    Ready,
    Downloading,
    Done,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxEntry {
    /// 說明檔在 Drive 上的 ID
    pub mid: String,
    pub m: Manifest,
    pub state: InboxState,
    pub path: Option<String>,
    pub error: Option<XferError>,
}

/// 給偏好設定的「收到的檔案」。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxOut {
    mid: String,
    name: String,
    size: u64,
    from: String,
    expires: u64,
    state: InboxState,
    error: Option<XferError>,
    has_file: bool,
}

impl InboxOut {
    pub fn mid(&self) -> &str {
        &self.mid
    }
    pub fn expired_now(&self) -> bool {
        self.state == InboxState::Expired || now_ms() >= self.expires
    }
}

#[derive(Default)]
pub struct Remote {
    devices: Mutex<Vec<DeviceInfo>>,
    inbox: Mutex<Option<Vec<InboxEntry>>>,
    /// 已經看過、不是傳給這台的說明檔
    skipped: Mutex<HashSet<String>>,
    published: Mutex<Option<Instant>>,
    devices_at: Mutex<Option<Instant>>,
    refreshed: Mutex<Option<Instant>>,
    wake: Notify,
}

impl Remote {
    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.devices.lock().clone()
    }

    /// 偏好設定打開時：太久沒檢查就馬上檢查一次。
    pub fn poke(&self) {
        if self.refreshed.lock().is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
            self.wake.notify_one();
        }
    }
}

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/* ---------------- 收件匣（存在本機，App 重開也看得到） ---------------- */

fn inbox_path(cloud: &Cloud) -> PathBuf {
    cloud.dir.join(INBOX_FILE)
}

fn with_inbox<T>(cloud: &Cloud, f: impl FnOnce(&mut Vec<InboxEntry>) -> T) -> T {
    let mut guard = cloud.transfers.remote.inbox.lock();
    let inbox = guard.get_or_insert_with(|| {
        std::fs::read(inbox_path(cloud)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    });
    let now = now_ms();
    for e in inbox.iter_mut() {
        if e.state == InboxState::Ready && e.m.expired(now) {
            e.state = InboxState::Expired;
        }
    }
    inbox.retain(|e| now < e.m.expires() + INBOX_KEEP_AFTER);
    let out = f(inbox);
    let _ = std::fs::write(inbox_path(cloud), serde_json::to_vec(inbox).unwrap_or_default());
    out
}

pub fn inbox(cloud: &Cloud) -> Vec<InboxOut> {
    with_inbox(cloud, |inbox| {
        let mut list: Vec<InboxOut> = inbox
            .iter()
            .map(|e| InboxOut {
                mid: e.mid.clone(),
                name: e.m.name.clone(),
                size: e.m.size,
                from: e.m.from.clone(),
                expires: e.m.expires(),
                state: e.state,
                error: e.error,
                has_file: e.path.as_ref().is_some_and(|p| Path::new(p).exists()),
            })
            .collect();
        list.sort_by(|a, b| b.expires.cmp(&a.expires));
        list
    })
}

/// 登出時清掉收件匣。
pub fn forget(cloud: &Cloud) {
    *cloud.transfers.remote.inbox.lock() = Some(Vec::new());
    let _ = std::fs::remove_file(inbox_path(cloud));
    cloud.transfers.remote.devices.lock().clear();
    *cloud.transfers.remote.published.lock() = None;
}

/// 新的檔案傳給這台：加進收件匣並通知使用者。回傳是不是新加的。
fn add_inbox(app: &AppHandle, mid: String, m: Manifest) -> bool {
    let cloud = app.state::<Cloud>();
    let added = with_inbox(&cloud, |inbox| {
        if inbox.iter().any(|e| e.mid == mid || e.m.id == m.id) {
            return false;
        }
        inbox.push(InboxEntry { mid: mid.clone(), m: m.clone(), state: InboxState::Ready, path: None, error: None });
        true
    });
    if added {
        let _ = app.emit("transfer:changed", ());
        crate::hud::incoming_file(app, &mid, &m.name, m.size, &m.from);
    }
    added
}

/* ---------------- 背景工作 ---------------- */

pub async fn run(app: AppHandle) {
    let cloud = app.state::<Cloud>();
    let mut config = cloud.config.subscribe();
    loop {
        let c = *config.borrow_and_update();
        if !c.signed_in {
            if config.changed().await.is_err() {
                return;
            }
            continue;
        }
        let _ = once(&app, &cloud).await;
        tokio::select! {
            _ = tokio::time::sleep(REFRESH) => {}
            _ = cloud.transfers.remote.wake.notified() => {}
            r = config.changed() => if r.is_err() { return },
        }
    }
}

/// 更新裝置資料、讀裝置清單、檢查收件匣、刪掉滿 24 小時的檔案。
async fn once(app: &AppHandle, cloud: &Cloud) -> Result<(), XferError> {
    let remote = &cloud.transfers.remote;
    *remote.refreshed.lock() = Some(Instant::now());
    let key = cloud.key().await?;
    let token = cloud.access_token().await?;
    let drive = Drive { http: &cloud.http, token: &token };
    let files = drive.list().await?;
    let now = now_ms();

    if remote.published.lock().is_none_or(|t| t.elapsed() > PUBLISH_EVERY) {
        let me = DeviceInfo {
            v: 1,
            id: cloud.device_id.clone(),
            name: cloud.device_name.clone(),
            platform: platform().into(),
            caps: CAP_FILES | CAP_DRIVE,
            seen: now,
        };
        if dx::publish_device(&drive, &files, &key, &me).await.is_ok() {
            *remote.published.lock() = Some(Instant::now());
            dx::prune_twins(&drive, &files, &key, &me, now).await;
        }
    }
    if remote.devices_at.lock().is_none_or(|t| t.elapsed() > DEVICES_EVERY) {
        let list = dx::read_devices(&drive, &files, &key, &cloud.device_id, now).await;
        *remote.devices.lock() = list;
        *remote.devices_at.lock() = Some(Instant::now());
        let _ = app.emit("transfer:changed", ());
    }

    // 收件匣：還沒看過的說明檔
    let known: HashSet<String> = with_inbox(cloud, |inbox| inbox.iter().map(|e| e.mid.clone()).collect());
    for f in files.iter().filter(|f| f.name.starts_with("x1-") && f.name.ends_with(".m")) {
        if known.contains(&f.id) || remote.skipped.lock().contains(&f.id) {
            continue;
        }
        match dx::get_manifest(&drive, &key, &f.id).await {
            Some(m) if for_me(cloud, &m) && !m.expired(now) => {
                add_inbox(app, f.id.clone(), m);
            }
            _ => {
                remote.skipped.lock().insert(f.id.clone());
            }
        }
    }

    // 滿 24 小時的刪掉（任何一台裝置都可以做）
    for id in dx::expired_files(&files, now) {
        let _ = drive.delete(&id).await;
    }
    let _ = app.emit("transfer:changed", ());
    Ok(())
}

fn for_me(cloud: &Cloud, m: &Manifest) -> bool {
    m.from_id != cloud.device_id && (m.to.is_empty() || m.to.contains(&cloud.device_id))
}

/// 即時同步收到「有檔案在 Drive 等你」的通知。
pub fn on_note(app: &AppHandle, note: FileNote) {
    let cloud = app.state::<Cloud>();
    if !note.to.is_empty() && !note.to.contains(&cloud.device_id) {
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

/* ---------------- 傳送 ---------------- */

/// 這個大小能不能經由 Drive 傳（使用者設定的上限）。
pub fn within_limit(app: &AppHandle, size: u64) -> bool {
    let mb = app.state::<AppState>().settings.lock().transfer_remote_mb;
    size <= mb.saturating_mul(1024 * 1024)
}

/// 把一個檔案經由 Drive 傳給 `peer`（`item` 是清單裡已經建立的那一筆）。
pub async fn send_one(app: &AppHandle, item: u64, cancel: &AtomicBool, path: &Path, name: &str, size: u64, peer: &str) {
    let cloud = app.state::<Cloud>();
    let transfers = &cloud.transfers;
    if !within_limit(app, size) {
        return transfers.fail(app, item, "tooBig");
    }
    transfers.set(app, item, |it| {
        it.state = TransferState::Active;
        it.via = Via::Drive;
        it.error = None;
    });
    let result: Result<Manifest, XferError> = async {
        let key = cloud.key().await?;
        let id: String = (0..12).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
        let token = || async { cloud.access_token().await.map_err(XferError::from) };
        let up = dx::upload(&cloud.http, &dx::GOOGLE, &key, token, &id, path, size, cancel, |n| {
            transfers.progress(app, item, n)
        })
        .await?;
        let m = Manifest {
            v: 1,
            id,
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
        // 通知對方（對方不在線上也沒關係，它下次檢查 Drive 時就會看到）
        if let Some(tx) = cloud.relay_tx.lock().clone() {
            let note = FileNote { m: mid, to: m.to.clone() };
            let _ = tx.send(wire::wire(&wire::seal_note(&key, &cloud.device_name, note), &[]));
        }
        Ok(m)
    }
    .await;
    match result {
        Ok(m) => transfers.set(app, item, |it| {
            it.state = TransferState::Done;
            it.done = it.size;
            it.expires = Some(m.expires());
        }),
        Err(XferError::Canceled) => {}
        Err(e) => transfers.fail(app, item, code(e)),
    }
}

pub fn code(e: XferError) -> &'static str {
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

/* ---------------- 下載 ---------------- */

/// 下載收件匣裡的一個檔案。超過 24 小時回傳 Expired。
pub async fn download(app: &AppHandle, mid: &str) -> Result<(), XferError> {
    let cloud = app.state::<Cloud>();
    let entry = with_inbox(&cloud, |inbox| inbox.iter().find(|e| e.mid == mid).cloned()).ok_or(XferError::Expired)?;
    if entry.m.expired(now_ms()) {
        set_entry(&cloud, mid, |e| e.state = InboxState::Expired);
        let _ = app.emit("transfer:changed", ());
        return Err(XferError::Expired);
    }
    if entry.state == InboxState::Downloading {
        return Ok(());
    }
    let m = entry.m;
    let rel = file_proto::safe_relative(&m.name).unwrap_or_else(|| PathBuf::from("CopyC file"));
    let dest = transfer::dest_dir(app);
    let partial_dir = dest.join(transfer::PARTIAL_DIR);
    tokio::fs::create_dir_all(&partial_dir).await?;
    let partial = partial_dir.join(format!("{}.dpart", m.id));
    let item = Item::new(Direction::In, Via::Drive, m.name.clone(), m.size, m.from.clone(), TransferState::Active);
    let (id, cancel) = cloud.transfers.add(item, None);
    set_entry(&cloud, mid, |e| {
        e.state = InboxState::Downloading;
        e.error = None;
    });
    let _ = app.emit("transfer:changed", ());

    let key = cloud.key().await?;
    let token = || async { cloud.access_token().await.map_err(XferError::from) };
    let result = dx::download(&cloud.http, &dx::GOOGLE, &key, token, &m, &partial, &cancel, |n| {
        cloud.transfers.progress(app, id, n)
    })
    .await;
    match result {
        Ok(()) => {
            let final_path = file_proto::unique_path(&dest, Path::new(rel.file_name().unwrap_or(rel.as_os_str())));
            if tokio::fs::rename(&partial, &final_path).await.is_err() {
                tokio::fs::copy(&partial, &final_path).await?;
                let _ = tokio::fs::remove_file(&partial).await;
            }
            let shown = final_path.to_string_lossy().into_owned();
            cloud.transfers.set(app, id, |it| {
                it.state = TransferState::Done;
                it.done = it.size;
                it.path = Some(shown.clone());
            });
            set_entry(&cloud, mid, |e| {
                e.state = InboxState::Done;
                e.path = Some(shown);
            });
            transfer::put_on_clipboard(app, vec![final_path], &m.from);
            Ok(())
        }
        Err(e) => {
            if e == XferError::Canceled {
                cloud.transfers.set(app, id, |it| it.state = TransferState::Canceled);
            } else {
                cloud.transfers.fail(app, id, code(e));
            }
            set_entry(&cloud, mid, |en| {
                en.state = if e == XferError::Expired { InboxState::Expired } else { InboxState::Ready };
                en.error = (e != XferError::Canceled).then_some(e);
            });
            let _ = app.emit("transfer:changed", ());
            Err(e)
        }
    }
}

fn set_entry(cloud: &Cloud, mid: &str, f: impl FnOnce(&mut InboxEntry)) {
    with_inbox(cloud, |inbox| {
        if let Some(e) = inbox.iter_mut().find(|e| e.mid == mid) {
            f(e);
        }
    });
}

pub fn inbox_path_of(app: &AppHandle, mid: &str) -> Option<PathBuf> {
    let cloud = app.state::<Cloud>();
    with_inbox(&cloud, |inbox| inbox.iter().find(|e| e.mid == mid).and_then(|e| e.path.clone()))
        .map(PathBuf::from)
        .filter(|p| p.exists())
}
