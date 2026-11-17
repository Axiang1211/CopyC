//! 跨網路傳檔（M3）：不在同一個網路時，檔案加密後放在使用者自己 Google Drive 的 App 專用資料夾，
//! 收件的裝置點「下載」才取回。電腦版與 Android 版共用這個檔案（Android 版以 #[path] 引用）。
//!
//! - 每個檔案在 Drive 上有兩個檔：`x1-<編號>.d`（內容）與 `x1-<編號>.m`（說明：檔名、大小、雜湊、寄件與收件裝置）。
//!   兩個都用帳號金鑰加密，Google 與我們都看不到檔名或內容。
//! - **保存 24 小時**：說明檔建立滿 24 小時後才由任何一台裝置刪除；收件端超過 24 小時就顯示「已過期」。
//! - 內容切成 4 MiB 一塊，每塊各自加密（帶塊序號），上傳用 Drive 的可續傳上傳，下載用 Range；
//!   斷線都能從中斷的地方接著傳。每塊的 nonce 由這次上傳的隨機鹽與塊序號算出，重傳同一塊時密文完全相同。
//! - 裝置清單：每台裝置寫一個 `d1-<裝置 ID>`（加密的名稱、平台、支援的功能），讓其他裝置知道可以傳給誰。

use super::{
    crypto::{Key, NONCE},
    drive::{Drive, File},
    CloudError,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    io::SeekFrom,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::{
    fs::{File as FsFile, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

/// Drive 的 API 位址（測試時換成本機的假伺服器）。
pub struct Api<'a> {
    pub upload: &'a str,
    pub files: &'a str,
}

pub const GOOGLE: Api<'static> =
    Api { upload: "https://www.googleapis.com/upload/drive/v3/files", files: "https://www.googleapis.com/drive/v3/files" };

/// 放在 Drive 上的檔案保留這麼久。
pub const KEEP: Duration = Duration::from_secs(24 * 3600);
/// 上傳到一半就中斷、沒有說明檔的內容，這麼久之後刪掉（很大的檔案可能要上傳好幾個小時）。
const ORPHAN_KEEP: Duration = Duration::from_secs(48 * 3600);
/// 裝置多久沒更新就不列出來。
pub const DEVICE_STALE: Duration = Duration::from_secs(60 * 24 * 3600);
/// 每塊的明文大小。
pub const CHUNK: usize = 4 << 20;
/// 每塊額外的長度：長度欄位 4 ＋ 加密標頭與驗證碼 44 ＋ 塊序號 8。
const OVERHEAD: u64 = 4 + 44 + 8;
pub const FRAME: u64 = CHUNK as u64 + OVERHEAD;
/// 每次上傳請求的大小（Drive 要求 256 KiB 的倍數）。
const PUT: usize = 16 << 20;
/// 網路錯誤時重試的次數（每次等久一點）。
const RETRIES: u32 = 8;

/// 傳送 / 下載失敗的原因（前端翻譯）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum XferError {
    Network,
    SignedOut,
    /// Google Drive 的空間不夠
    Quota,
    /// 已經超過 24 小時被刪掉了
    Expired,
    /// 內容比對不符
    Corrupt,
    /// 本機讀寫失敗（例如空間不足、檔案被移走）
    Disk,
    Canceled,
}

impl From<CloudError> for XferError {
    fn from(e: CloudError) -> Self {
        match e {
            CloudError::SignedOut => XferError::SignedOut,
            _ => XferError::Network,
        }
    }
}

impl From<std::io::Error> for XferError {
    fn from(_: std::io::Error) -> Self {
        XferError::Disk
    }
}

pub fn manifest_name(id: &str) -> String {
    format!("x1-{id}.m")
}
pub fn data_name(id: &str) -> String {
    format!("x1-{id}.d")
}
pub fn device_file(id: &str) -> String {
    format!("d1-{id}")
}

/// 一個放在 Drive 等待下載的檔案。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub v: u32,
    pub id: String,
    pub name: String,
    pub size: u64,
    /// 整個檔案的 SHA-256（十六進位）
    pub sha: String,
    /// 內容檔在 Drive 上的 ID
    pub data: String,
    pub from: String,
    pub from_id: String,
    pub to: Vec<String>,
    /// 上傳完成的時間（毫秒）；過了 [`KEEP`] 就過期
    pub at: u64,
}

impl Manifest {
    pub fn expires(&self) -> u64 {
        self.at + KEEP.as_millis() as u64
    }
    pub fn expired(&self, now: u64) -> bool {
        now >= self.expires()
    }
}

/// 裝置清單裡的一台裝置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceInfo {
    pub v: u32,
    pub id: String,
    pub name: String,
    /// windows / macos / android / ios
    pub platform: String,
    /// 支援的功能（同 lan_proto 的 CAP_*）
    #[serde(default)]
    pub caps: u32,
    /// 最後一次更新（毫秒）
    pub seen: u64,
}

pub fn seal_json<T: Serialize>(key: &Key, value: &T) -> Vec<u8> {
    key.seal(&serde_json::to_vec(value).unwrap_or_default())
}

pub fn open_json<T: DeserializeOwned>(key: &Key, data: &[u8]) -> Option<T> {
    serde_json::from_slice(&key.open(data)?).ok()
}

/* ---------------- 內容格式 ---------------- */

/// 檔案切成幾塊（空檔案也有一塊）。
pub fn chunks(size: u64) -> u64 {
    size.div_ceil(CHUNK as u64).max(1)
}

/// 加密後的內容檔大小。
pub fn blob_len(size: u64) -> u64 {
    size + chunks(size) * OVERHEAD
}

fn nonce(salt: &[u8; 32], index: u64) -> [u8; NONCE] {
    let mut h = Sha256::new();
    h.update(salt);
    h.update(index.to_be_bytes());
    let mut out = [0u8; NONCE];
    out.copy_from_slice(&h.finalize()[..NONCE]);
    out
}

/// 一塊加密後的內容：長度（4）＋ 加密（塊序號 8 ＋ 明文）。
pub fn frame(key: &Key, salt: &[u8; 32], index: u64, plain: &[u8]) -> Vec<u8> {
    let mut inner = Vec::with_capacity(8 + plain.len());
    inner.extend_from_slice(&index.to_be_bytes());
    inner.extend_from_slice(plain);
    let sealed = key.seal_with_nonce(&nonce(salt, index), &inner);
    let mut out = Vec::with_capacity(4 + sealed.len());
    out.extend_from_slice(&(sealed.len() as u32).to_be_bytes());
    out.extend_from_slice(&sealed);
    out
}

/// 把下載到的位元組依序解成一塊一塊的明文。
pub struct Decoder {
    buf: Vec<u8>,
    /// 下一塊應該是第幾塊
    pub next: u64,
}

impl Decoder {
    pub fn new(next: u64) -> Self {
        Decoder { buf: Vec::new(), next }
    }

    /// 餵進一段位元組，回傳已經完整的幾塊明文。順序錯了或解不開回傳 Err。
    pub fn push(&mut self, key: &Key, bytes: &[u8]) -> Result<Vec<Vec<u8>>, XferError> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            if self.buf.len() < 4 {
                break;
            }
            let len = u32::from_be_bytes(self.buf[..4].try_into().unwrap_or_default()) as usize;
            if len as u64 > FRAME {
                return Err(XferError::Corrupt);
            }
            if self.buf.len() < 4 + len {
                break;
            }
            let inner = key.open(&self.buf[4..4 + len]).ok_or(XferError::Corrupt)?;
            if inner.len() < 8 || u64::from_be_bytes(inner[..8].try_into().unwrap_or_default()) != self.next {
                return Err(XferError::Corrupt);
            }
            self.buf.drain(..4 + len);
            self.next += 1;
            out.push(inner[8..].to_vec());
        }
        Ok(out)
    }
}

/* ---------------- 上傳 ---------------- */

fn status_error(code: u16, body: &str) -> XferError {
    match code {
        401 => XferError::SignedOut,
        403 if body.contains("storageQuotaExceeded") || body.contains("quotaExceeded") => XferError::Quota,
        404 | 410 => XferError::Expired,
        _ => XferError::Network,
    }
}

enum UpState {
    At(u64),
    Done(String),
}

async fn start_upload(http: &reqwest::Client, api: &Api<'_>, token: &str, name: &str, total: u64) -> Result<String, XferError> {
    let meta = serde_json::json!({ "name": name, "parents": ["appDataFolder"] });
    let resp = http
        .post(api.upload)
        .bearer_auth(token)
        .query(&[("uploadType", "resumable"), ("fields", "id")])
        .header("X-Upload-Content-Type", "application/octet-stream")
        .header("X-Upload-Content-Length", total.to_string())
        .json(&meta)
        .send()
        .await
        .map_err(|_| XferError::Network)?;
    let code = resp.status().as_u16();
    if !(200..300).contains(&code) {
        let body = resp.text().await.unwrap_or_default();
        return Err(status_error(code, &body));
    }
    resp.headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .ok_or(XferError::Network)
}

async fn up_state(resp: reqwest::Response) -> Result<UpState, XferError> {
    let code = resp.status().as_u16();
    match code {
        200 | 201 => {
            #[derive(Deserialize)]
            struct Created {
                id: String,
            }
            let created: Created = resp.json().await.map_err(|_| XferError::Network)?;
            Ok(UpState::Done(created.id))
        }
        308 => {
            // Range: bytes=0-123 表示收到 0～123；沒有這個標頭表示還沒收到任何東西
            let next = resp
                .headers()
                .get("range")
                .and_then(|v| v.to_str().ok())
                .and_then(|r| r.rsplit('-').next())
                .and_then(|n| n.parse::<u64>().ok())
                .map_or(0, |n| n + 1);
            Ok(UpState::At(next))
        }
        _ => {
            let body = resp.text().await.unwrap_or_default();
            Err(status_error(code, &body))
        }
    }
}

async fn put_range(
    http: &reqwest::Client,
    token: &str,
    session: &str,
    start: u64,
    data: Vec<u8>,
    total: u64,
) -> Result<UpState, XferError> {
    let range = if data.is_empty() {
        format!("bytes */{total}")
    } else {
        format!("bytes {}-{}/{total}", start, start + data.len() as u64 - 1)
    };
    let resp = http
        .put(session)
        .bearer_auth(token)
        .header("Content-Range", range)
        .body(data)
        .send()
        .await
        .map_err(|_| XferError::Network)?;
    up_state(resp).await
}

/// 讀檔案的第 `index` 塊。
async fn read_chunk(file: &mut FsFile, size: u64, index: u64) -> Result<Vec<u8>, XferError> {
    let start = index * CHUNK as u64;
    let len = size.saturating_sub(start).min(CHUNK as u64) as usize;
    let mut buf = vec![0u8; len];
    file.seek(SeekFrom::Start(start)).await?;
    file.read_exact(&mut buf).await?;
    Ok(buf)
}

/// 上傳完成：內容檔的 Drive ID 與整個檔案的 SHA-256。
pub struct Uploaded {
    pub data: String,
    pub sha: String,
}

/// 把檔案加密上傳成 `x1-<id>.d`。`token` 每次需要時呼叫（長時間上傳時 access token 會換新）。
/// `progress` 收到已經上傳的明文位元組數（大約）。
pub async fn upload<T, F>(
    http: &reqwest::Client,
    api: &Api<'_>,
    key: &Key,
    token: T,
    id: &str,
    path: &Path,
    size: u64,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<Uploaded, XferError>
where
    T: Fn() -> F,
    F: Future<Output = Result<String, XferError>>,
{
    let total = blob_len(size);
    let salt: [u8; 32] = rand::random();
    let mut file = FsFile::open(path).await?;
    let mut hasher = Sha256::new();
    let mut hashed = 0u64;
    let mut session = start_upload(http, api, &token().await?, &data_name(id), total).await?;
    let mut pos = 0u64;
    let mut failures = 0u32;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = http.delete(&session).send().await;
            return Err(XferError::Canceled);
        }
        // 從 pos 開始組出下一段（最多 PUT 位元組）
        let mut out = Vec::with_capacity(PUT + FRAME as usize);
        let mut index = pos / FRAME;
        let mut skip = (pos % FRAME) as usize;
        while out.len() < PUT && index < chunks(size) {
            let plain = read_chunk(&mut file, size, index).await?;
            if index == hashed {
                hasher.update(&plain);
                hashed += 1;
            }
            let f = frame(key, &salt, index, &plain);
            out.extend_from_slice(&f[skip.min(f.len())..]);
            skip = 0;
            index += 1;
        }
        out.truncate(PUT);
        let result = match token().await {
            Ok(t) => put_range(http, &t, &session, pos, out, total).await,
            Err(e) => Err(e),
        };
        match result {
            Ok(UpState::Done(data)) => {
                // 最後幾塊可能在前一輪就算過雜湊；保險起見補齊沒算到的部分
                while hashed < chunks(size) {
                    hasher.update(read_chunk(&mut file, size, hashed).await?);
                    hashed += 1;
                }
                progress(size);
                let sha = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
                return Ok(Uploaded { data, sha });
            }
            Ok(UpState::At(next)) => {
                failures = 0;
                pos = next;
                progress(((pos as u128 * size as u128) / total.max(1) as u128) as u64);
            }
            Err(e @ (XferError::Quota | XferError::SignedOut | XferError::Disk)) => return Err(e),
            Err(XferError::Expired) => {
                // 上傳工作階段失效：從頭開始（同一個鹽，密文一樣）
                session = start_upload(http, api, &token().await?, &data_name(id), total).await?;
                pos = 0;
            }
            Err(e) => {
                failures += 1;
                if failures > RETRIES {
                    return Err(e);
                }
                tokio::time::sleep(Duration::from_secs(2u64.pow(failures.min(5)))).await;
                // 問 Drive 收到哪裡了，從那裡接著傳
                if let Ok(t) = token().await {
                    match put_range(http, &t, &session, 0, Vec::new(), total).await {
                        Ok(UpState::At(next)) => pos = next,
                        Ok(UpState::Done(data)) => {
                            while hashed < chunks(size) {
                                hasher.update(read_chunk(&mut file, size, hashed).await?);
                                hashed += 1;
                            }
                            let sha = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
                            return Ok(Uploaded { data, sha });
                        }
                        Err(_) => {}
                    }
                }
            }
        }
    }
}

/* ---------------- 下載 ---------------- */

/// 下載並解密到 `partial`（已經有一部分就接著下載）。成功時檔案完整、雜湊相符。
pub async fn download<T, F>(
    http: &reqwest::Client,
    api: &Api<'_>,
    key: &Key,
    token: T,
    m: &Manifest,
    partial: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<(), XferError>
where
    T: Fn() -> F,
    F: Future<Output = Result<String, XferError>>,
{
    let mut failures = 0u32;
    loop {
        let mut file = OpenOptions::new().create(true).read(true).write(true).truncate(false).open(partial).await?;
        // 只保留完整的塊，從下一塊接著下載
        let have = file.metadata().await?.len().min(m.size);
        let done = if have == m.size && m.size > 0 { chunks(m.size) } else { have / CHUNK as u64 };
        let keep = (done * CHUNK as u64).min(m.size);
        file.set_len(keep).await?;
        let mut hasher = Sha256::new();
        file.seek(SeekFrom::Start(0)).await?;
        let mut buf = vec![0u8; CHUNK];
        let mut pos = 0u64;
        while pos < keep {
            let want = ((keep - pos) as usize).min(CHUNK);
            file.read_exact(&mut buf[..want]).await?;
            hasher.update(&buf[..want]);
            pos += want as u64;
        }
        progress(keep);
        let start = done * FRAME;
        let mut written = keep;
        let result: Result<(), XferError> = async {
            if done < chunks(m.size) {
                let resp = http
                    .get(format!("{}/{}", api.files, m.data))
                    .bearer_auth(token().await?)
                    .query(&[("alt", "media")])
                    .header("Range", format!("bytes={start}-"))
                    .send()
                    .await
                    .map_err(|_| XferError::Network)?;
                let code = resp.status().as_u16();
                if !(200..300).contains(&code) {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(status_error(code, &body));
                }
                // 伺服器沒照 Range 回應（200）：前面的部分自己略過
                let mut skip = if code == 200 { start } else { 0 };
                let mut decoder = Decoder::new(done);
                let mut resp = resp;
                while let Some(bytes) = resp.chunk().await.map_err(|_| XferError::Network)? {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(XferError::Canceled);
                    }
                    let bytes = if skip > 0 {
                        let s = (skip as usize).min(bytes.len());
                        skip -= s as u64;
                        bytes.slice(s..)
                    } else {
                        bytes
                    };
                    for plain in decoder.push(key, &bytes)? {
                        if written + plain.len() as u64 > m.size {
                            return Err(XferError::Corrupt);
                        }
                        file.write_all(&plain).await?;
                        hasher.update(&plain);
                        written += plain.len() as u64;
                        progress(written);
                    }
                }
                if decoder.next < chunks(m.size) {
                    return Err(XferError::Network);
                }
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                file.flush().await?;
                file.sync_all().await?;
                let sha: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
                if written != m.size || sha != m.sha {
                    drop(file);
                    let _ = tokio::fs::remove_file(partial).await;
                    return Err(XferError::Corrupt);
                }
                return Ok(());
            }
            Err(XferError::Network) => {
                let _ = file.flush().await;
                failures += 1;
                if failures > RETRIES || cancel.load(Ordering::Relaxed) {
                    return Err(if cancel.load(Ordering::Relaxed) { XferError::Canceled } else { XferError::Network });
                }
                tokio::time::sleep(Duration::from_secs(2u64.pow(failures.min(5)))).await;
            }
            Err(XferError::Corrupt) => {
                drop(file);
                let _ = tokio::fs::remove_file(partial).await;
                return Err(XferError::Corrupt);
            }
            Err(e) => return Err(e),
        }
    }
}

/* ---------------- 說明檔、裝置清單與清理 ---------------- */

/// 上傳說明檔，回傳它在 Drive 上的 ID。
pub async fn put_manifest(drive: &Drive<'_>, key: &Key, m: &Manifest) -> Result<String, CloudError> {
    drive.create(&manifest_name(&m.id), seal_json(key, m)).await
}

pub async fn get_manifest(drive: &Drive<'_>, key: &Key, file_id: &str) -> Option<Manifest> {
    open_json(key, &drive.download(file_id).await.ok()?)
}

/// 寫入（或更新）這台裝置的資料。
pub async fn publish_device(drive: &Drive<'_>, files: &[File], key: &Key, me: &DeviceInfo) -> Result<(), CloudError> {
    let name = device_file(&me.id);
    let mut mine: Vec<&File> = files.iter().filter(|f| f.name == name).collect();
    mine.sort_by(|a, b| a.created_time.cmp(&b.created_time));
    match mine.first() {
        Some(f) => {
            drive.update(&f.id, seal_json(key, me)).await?;
            for extra in &mine[1..] {
                let _ = drive.delete(&extra.id).await;
            }
            Ok(())
        }
        None => drive.create(&name, seal_json(key, me)).await.map(drop),
    }
}

/// 讀取其他裝置的資料（不含自己、太久沒更新的）。
pub async fn read_devices(drive: &Drive<'_>, files: &[File], key: &Key, me: &str, now: u64) -> Vec<DeviceInfo> {
    let mut out: Vec<DeviceInfo> = Vec::new();
    for f in files.iter().filter(|f| f.name.starts_with("d1-") && f.name != device_file(me)) {
        let Ok(raw) = drive.download(&f.id).await else { continue };
        let Some(d) = open_json::<DeviceInfo>(key, &raw) else { continue };
        if now.saturating_sub(d.seen) < DEVICE_STALE.as_millis() as u64 && !out.iter().any(|o| o.id == d.id) {
            out.push(d);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    out
}

/// 「2026-11-17T05:05:22.123Z」→ 毫秒。
pub fn rfc3339_ms(s: &str) -> Option<u64> {
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, mo, da) = (d.next()??, d.next()??, d.next()??);
    let time = time.trim_end_matches('Z');
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.split(':').map(|p| p.parse::<i64>().ok());
    let (h, mi, se) = (t.next()??, t.next()??, t.next()??);
    let ms: i64 = format!("{:0<3}", &frac[..frac.len().min(3)]).parse().ok()?;
    // 年月日 → 1970-01-01 起算的日數（Howard Hinnant 的演算法）
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let total = ((days * 24 + h) * 60 + mi) * 60 + se;
    u64::try_from(total * 1000 + ms).ok()
}

/// 該刪掉的跨網路傳檔：說明檔建立滿 24 小時（連同它的內容檔），以及沒有說明檔、放太久的內容檔。
pub fn expired_files(files: &[File], now: u64) -> Vec<String> {
    let age = |f: &File| rfc3339_ms(&f.created_time).map_or(0, |c| now.saturating_sub(c));
    let mut out = Vec::new();
    for f in files.iter().filter(|f| f.name.starts_with("x1-")) {
        let Some(stem) = f.name.strip_suffix(".m").or_else(|| f.name.strip_suffix(".d")) else { continue };
        let manifest = files.iter().find(|m| m.name == format!("{stem}.m"));
        let old = match manifest {
            Some(m) => age(m) >= KEEP.as_millis() as u64,
            None => age(f) >= ORPHAN_KEEP.as_millis() as u64,
        };
        if old {
            out.push(f.id.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_decode_in_order_and_resend_identically() {
        let key = Key::generate();
        let salt: [u8; 32] = rand::random();
        let a = frame(&key, &salt, 0, b"hello");
        assert_eq!(a, frame(&key, &salt, 0, b"hello"), "同一塊重傳時密文相同（可續傳上傳需要）");
        assert_ne!(a, frame(&key, &rand::random(), 0, b"hello"));
        let b = frame(&key, &salt, 1, b" world");
        assert_eq!(a.len() as u64, 5 + OVERHEAD);
        let mut stream = a.clone();
        stream.extend_from_slice(&b);
        // 一次餵一點點也能正確切塊
        let mut d = Decoder::new(0);
        let mut got = Vec::new();
        for piece in stream.chunks(7) {
            got.extend(d.push(&key, piece).unwrap());
        }
        assert_eq!(got, vec![b"hello".to_vec(), b" world".to_vec()]);
        // 順序錯了、金鑰不對都會被發現
        assert_eq!(Decoder::new(0).push(&key, &b), Err(XferError::Corrupt));
        assert_eq!(Decoder::new(0).push(&Key::generate(), &a), Err(XferError::Corrupt));
    }

    #[test]
    fn blob_sizes() {
        assert_eq!(chunks(0), 1);
        assert_eq!(chunks(1), 1);
        assert_eq!(chunks(CHUNK as u64), 1);
        assert_eq!(chunks(CHUNK as u64 + 1), 2);
        assert_eq!(blob_len(0), OVERHEAD);
        assert_eq!(blob_len(CHUNK as u64 * 3), FRAME * 3);
        let key = Key::generate();
        assert_eq!(frame(&key, &[0; 32], 0, &vec![0u8; CHUNK]).len() as u64, FRAME);
        assert_eq!(PUT % (256 * 1024), 0);
    }

    #[test]
    fn manifests_and_devices_are_encrypted() {
        let key = Key::generate();
        let m = Manifest {
            v: 1,
            id: "abc".into(),
            name: "報告.pdf".into(),
            size: 10,
            sha: "00".into(),
            data: "drive-id".into(),
            from: "MY-PC".into(),
            from_id: "0123456789abcdef".into(),
            to: vec!["fedcba9876543210".into()],
            at: 1_000,
        };
        let sealed = seal_json(&key, &m);
        assert!(!String::from_utf8_lossy(&sealed).contains("報告"));
        assert_eq!(open_json::<Manifest>(&key, &sealed), Some(m.clone()));
        assert!(open_json::<Manifest>(&Key::generate(), &sealed).is_none());
        assert!(!m.expired(1_000 + KEEP.as_millis() as u64 - 1));
        assert!(m.expired(1_000 + KEEP.as_millis() as u64));
    }

    #[test]
    fn parses_drive_times() {
        assert_eq!(rfc3339_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(rfc3339_ms("2026-11-17T05:05:22.123Z"), Some(1_794_891_922_123));
        assert_eq!(rfc3339_ms("2026-11-17T05:05:22Z"), Some(1_794_891_922_000));
        assert_eq!(rfc3339_ms("2024-02-29T12:00:00.5Z"), Some(1_709_208_000_500));
        assert_eq!(rfc3339_ms("garbage"), None);
    }

    #[test]
    fn deletes_after_24_hours_only() {
        let f = |id: &str, name: &str, created: &str| File {
            id: id.into(),
            name: name.into(),
            modified_time: created.into(),
            created_time: created.into(),
        };
        let now = rfc3339_ms("2026-11-18T12:00:00Z").unwrap();
        let files = vec![
            // 說明檔 23 小時前：保留（內容檔雖然更早開始上傳也保留）
            f("1", "x1-a.m", "2026-11-17T13:00:00Z"),
            f("2", "x1-a.d", "2026-11-17T09:00:00Z"),
            // 說明檔滿 24 小時：兩個都刪
            f("3", "x1-b.m", "2026-11-17T12:00:00Z"),
            f("4", "x1-b.d", "2026-11-17T11:00:00Z"),
            // 沒有說明檔：上傳中斷，48 小時後才刪
            f("5", "x1-c.d", "2026-11-17T00:00:00Z"),
            f("6", "x1-e.d", "2026-11-16T11:00:00Z"),
            // 其他檔案不動
            f("7", "h1-dev-2026-11-01", "2026-11-01T00:00:00Z"),
        ];
        let mut got = expired_files(&files, now);
        got.sort();
        assert_eq!(got, ["3", "4", "6"]);
    }

    /// 本機的假 Drive：可續傳上傳與 Range 下載，並且故意斷線一次。
    mod fake {
        use std::sync::{Arc, Mutex};
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::{TcpListener, TcpStream},
        };

        #[derive(Default)]
        pub struct State {
            pub blob: Vec<u8>,
            pub total: u64,
            pub puts: u32,
            pub gets: u32,
        }

        async fn read_request(s: &mut TcpStream) -> Option<(String, String, Vec<(String, String)>, Vec<u8>)> {
            let mut buf = Vec::new();
            let mut byte = [0u8; 1];
            while !buf.ends_with(b"\r\n\r\n") {
                if s.read(&mut byte).await.ok()? == 0 {
                    return None;
                }
                buf.push(byte[0]);
            }
            let head = String::from_utf8(buf).ok()?;
            let mut lines = head.split("\r\n");
            let mut first = lines.next()?.split(' ');
            let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
            let headers: Vec<(String, String)> = lines
                .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())))
                .collect();
            let len = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
            let mut body = vec![0u8; len];
            s.read_exact(&mut body).await.ok()?;
            Some((method, path, headers, body))
        }

        pub async fn serve() -> (String, Arc<Mutex<State>>) {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", l.local_addr().unwrap());
            let state = Arc::new(Mutex::new(State::default()));
            let (st, b2) = (state.clone(), base.clone());
            tokio::spawn(async move {
                loop {
                    let (mut s, _) = l.accept().await.unwrap();
                    let Some((method, path, headers, body)) = read_request(&mut s).await else { continue };
                    let h = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
                    let mut out: Vec<u8>;
                    {
                        let mut st = st.lock().unwrap();
                        if method == "POST" {
                            st.total = h("x-upload-content-length").parse().unwrap();
                            st.blob.clear();
                            out = format!("HTTP/1.1 200 OK\r\nLocation: {b2}/session\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes();
                        } else if method == "PUT" {
                            st.puts += 1;
                            let range = h("content-range");
                            if let Some(start) = range.strip_prefix("bytes ").and_then(|r| r.split('-').next()).and_then(|a| a.parse::<usize>().ok()) {
                                if start == st.blob.len() {
                                    if st.puts == 2 {
                                        // 第二次上傳只收到一部分就斷線（停在某一塊的中間）
                                        st.blob.extend_from_slice(&body[..1000]);
                                        continue;
                                    }
                                    st.blob.extend_from_slice(&body);
                                }
                            }
                            out = if st.blob.len() as u64 == st.total {
                                let j = r#"{"id":"blob1"}"#;
                                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{j}", j.len()).into_bytes()
                            } else if st.blob.is_empty() {
                                b"HTTP/1.1 308 Resume Incomplete\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                            } else {
                                format!("HTTP/1.1 308 Resume Incomplete\r\nRange: bytes=0-{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", st.blob.len() - 1).into_bytes()
                            };
                        } else if path.starts_with("/files/blob1") {
                            st.gets += 1;
                            let from: usize = h("range").trim_start_matches("bytes=").trim_end_matches('-').parse().unwrap_or(0);
                            let mut part = st.blob[from..].to_vec();
                            if st.gets == 1 {
                                // 第一次下載只給一部分（連線中斷）
                                part.truncate(5 << 20);
                            }
                            out = format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", part.len()).into_bytes();
                            out.extend_from_slice(&part);
                        } else {
                            out = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
                        }
                    }
                    let _ = s.write_all(&out).await;
                    let _ = s.shutdown().await;
                }
            });
            (base, state)
        }
    }

    #[tokio::test]
    async fn upload_and_download_survive_dropped_connections() {
        let (base, state) = fake::serve().await;
        let upload_url = format!("{base}/upload");
        let files_url = format!("{base}/files");
        let api = Api { upload: &upload_url, files: &files_url };
        let http = reqwest::Client::new();
        let key = Key::generate();
        let dir = std::env::temp_dir().join(format!("copyc-dx-{}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let size = 40 * 1024 * 1024 + 12_345;
        let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let src = dir.join("src.bin");
        std::fs::write(&src, &data).unwrap();
        let cancel = AtomicBool::new(false);
        let token = || async { Ok::<_, XferError>("t".to_string()) };

        let up = upload(&http, &api, &key, token, "abc", &src, size as u64, &cancel, |_| {}).await.unwrap();
        assert_eq!(up.data, "blob1");
        assert_eq!(state.lock().unwrap().blob.len() as u64, blob_len(size as u64));
        assert!(state.lock().unwrap().puts >= 4, "中斷後有問進度並接著傳");
        let expected: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(up.sha, expected);

        let m = Manifest {
            v: 1,
            id: "abc".into(),
            name: "a.bin".into(),
            size: size as u64,
            sha: up.sha,
            data: up.data,
            from: "PC".into(),
            from_id: "x".into(),
            to: vec![],
            at: 0,
        };
        let out = dir.join("out.part");
        let mut seen = Vec::new();
        download(&http, &api, &key, token, &m, &out, &cancel, |n| seen.push(n)).await.unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), data);
        assert_eq!(state.lock().unwrap().gets, 2, "第一次下載中斷，從完整的塊接著下載");
        assert!(seen.contains(&(CHUNK as u64)));

        // 下載到一半的檔案被改過：最後比對失敗、刪掉
        std::fs::write(&out, vec![0u8; CHUNK]).unwrap();
        assert_eq!(download(&http, &api, &key, token, &m, &out, &cancel, |_| {}).await, Err(XferError::Corrupt));
        assert!(!out.exists());
        // 過期（Drive 上已經刪掉）
        let gone = Manifest { data: "nope".into(), ..m };
        assert_eq!(download(&http, &api, &key, token, &gone, &out, &cancel, |_| {}).await, Err(XferError::Expired));
        let _ = std::fs::remove_dir_all(dir);
    }
}
