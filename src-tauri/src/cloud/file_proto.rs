//! 檔案互傳的協定（M1：同一個網路裡、同一個帳號的電腦之間）。不依賴 Tauri，之後 Android 版可以用 #[path] 引用。
//!
//! 連線沿用區域網路直傳的 TCP 埠，開頭 4 個位元組是 `CPF1`（舊版把它當成長度，超過上限就直接斷線，不會誤收）。
//! 之後每個訊框都是「長度（4 位元組）＋ 用帳號金鑰加密的內容」：
//!
//! 1. 傳送端 → [`Offer`]：檔名（可含子資料夾）、大小、這次傳送的編號。
//! 2. 接收端 → [`Reply`]：已經收到多少位元組（中斷後重傳時從這裡接著傳），或拒絕的原因。
//! 3. 傳送端 → 資料塊：`0x01` ＋ 位移（8 位元組）＋ 最多 1 MiB 內容；最後 `0x02` ＋ 整個檔案的 SHA-256。
//! 4. 接收端 → [`Done`]：比對雜湊後回覆成功或失敗。
//!
//! 每一塊都經過驗證加密，而且帶著位移，順序錯了、被截斷或被竄改都會被發現。檔案大小沒有上限。

use super::crypto::Key;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io,
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
};

pub const MAGIC: &[u8; 4] = b"CPF1";
/// 每一塊的明文大小。
pub const CHUNK: usize = 1 << 20;
/// 控制訊息（Offer / Reply / Done）的上限。
const MAX_CTRL: usize = 64 * 1024;
/// 資料訊框上限：一塊內容 ＋ 類型與位移 ＋ 加密的額外長度。
const MAX_DATA: usize = CHUNK + 9 + 64;
/// 對方這麼久沒有任何動靜就當作斷線（之後由傳送端自動重試、接著傳）。
pub const STALL: Duration = Duration::from_secs(30);
/// Offer 的時間和現在差太多就不接受（防重播）。
pub const MAX_AGE: Duration = Duration::from_secs(10 * 60);

const DATA: u8 = 1;
const END: u8 = 2;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// 傳送端提出要傳的檔案。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Offer {
    pub v: u32,
    /// 這個檔案的傳送編號：同一台電腦、同一個檔案（路徑、大小、修改時間都相同）每次都一樣，中斷後才接得上。
    pub id: String,
    /// 檔名；整個資料夾一起傳時是以 `/` 分隔的相對路徑。
    pub name: String,
    pub size: u64,
    /// 傳送端的裝置名稱與 ID
    pub from: String,
    pub from_id: String,
    pub at: u64,
    /// 同一批（一次拖進來的所有檔案）共用的編號、這是第幾個、總共幾個。
    #[serde(default)]
    pub batch: String,
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub count: u32,
}

impl Offer {
    pub fn new(id: String, name: String, size: u64, from: String, from_id: String) -> Self {
        Offer { v: 1, id, name, size, from, from_id, at: now_ms(), batch: String::new(), index: 0, count: 1 }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reply {
    /// 已經收到的位元組數
    pub offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Done {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// 同一個檔案每次都算出同樣的傳送編號。
pub fn transfer_id(device_id: &str, path: &Path, size: u64, modified: u64) -> String {
    let mut h = Sha256::new();
    h.update(device_id.as_bytes());
    h.update([0]);
    h.update(path.to_string_lossy().as_bytes());
    h.update([0]);
    h.update(size.to_be_bytes());
    h.update(modified.to_be_bytes());
    h.finalize()[..12].iter().map(|b| format!("{b:02x}")).collect()
}

fn valid_id(id: &str) -> bool {
    (16..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 把對方給的檔名變成安全的相對路徑：不能跳出接收資料夾、不能是絕對路徑或 Windows 的保留名稱。
pub fn safe_relative(name: &str) -> Option<PathBuf> {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
        "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let mut out = PathBuf::new();
    for raw in name.split('/') {
        let cleaned: String = raw
            .chars()
            .map(|c| if c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
            .collect();
        // Windows 不允許結尾是空白或句點
        let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
        if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
            return None;
        }
        let stem = cleaned.split('.').next().unwrap_or_default().to_ascii_uppercase();
        let cleaned = if RESERVED.contains(&stem.as_str()) { format!("_{cleaned}") } else { cleaned };
        let cleaned: String = cleaned.chars().take(200).collect();
        out.push(cleaned);
    }
    let ok = out.components().count() > 0 && out.components().all(|c| matches!(c, Component::Normal(_)));
    ok.then_some(out)
}

async fn write_sealed<W: AsyncWrite + Unpin>(w: &mut W, key: &Key, plain: &[u8]) -> io::Result<()> {
    let sealed = key.seal(plain);
    w.write_all(&(sealed.len() as u32).to_be_bytes()).await?;
    w.write_all(&sealed).await
}

async fn read_sealed<R: AsyncRead + Unpin>(r: &mut R, key: &Key, max: usize) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    tokio::time::timeout(STALL, r.read_exact(&mut len)).await.map_err(|_| io::ErrorKind::TimedOut)??;
    let len = u32::from_be_bytes(len) as usize;
    if len > max {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut buf = vec![0u8; len];
    tokio::time::timeout(STALL, r.read_exact(&mut buf)).await.map_err(|_| io::ErrorKind::TimedOut)??;
    key.open(&buf).ok_or_else(|| io::ErrorKind::InvalidData.into())
}

async fn write_json<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, key: &Key, value: &T) -> io::Result<()> {
    write_sealed(w, key, &serde_json::to_vec(value).map_err(io::Error::other)?).await
}

async fn read_json<R: AsyncRead + Unpin, T: for<'de> Deserialize<'de>>(r: &mut R, key: &Key) -> io::Result<T> {
    serde_json::from_slice(&read_sealed(r, key, MAX_CTRL).await?).map_err(|_| io::ErrorKind::InvalidData.into())
}

/// 傳送失敗的原因。
#[derive(Debug)]
pub enum SendError {
    /// 連線、讀寫失敗或對方沒有回應：可以稍後接著傳
    Io(io::Error),
    /// 對方拒絕（例如磁碟空間不足）
    Rejected(String),
    /// 對方收完後比對雜湊不符：重新從頭傳
    Corrupt,
}

impl From<io::Error> for SendError {
    fn from(e: io::Error) -> Self {
        SendError::Io(e)
    }
}

/// 把一個檔案送到已經連上的串流（呼叫端先送出 [`MAGIC`]）。`progress` 收到目前已送出的位元組數。
pub async fn send<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    key: &Key,
    offer: &Offer,
    path: &Path,
    mut progress: impl FnMut(u64),
) -> Result<(), SendError> {
    write_json(stream, key, offer).await?;
    let reply: Reply = read_json(stream, key).await?;
    if let Some(reason) = reply.reject {
        return Err(SendError::Rejected(reason));
    }
    let start = reply.offset.min(offer.size);
    let mut file = File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    // 對方已經有的部分：只算雜湊、不重送
    let mut pos = 0u64;
    while pos < start {
        let want = ((start - pos) as usize).min(CHUNK);
        file.read_exact(&mut buf[..want]).await?;
        hasher.update(&buf[..want]);
        pos += want as u64;
    }
    progress(pos);
    let mut frame = Vec::with_capacity(CHUNK + 9);
    while pos < offer.size {
        let want = ((offer.size - pos) as usize).min(CHUNK);
        file.read_exact(&mut buf[..want]).await?;
        hasher.update(&buf[..want]);
        frame.clear();
        frame.push(DATA);
        frame.extend_from_slice(&pos.to_be_bytes());
        frame.extend_from_slice(&buf[..want]);
        tokio::time::timeout(STALL, write_sealed(stream, key, &frame))
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        pos += want as u64;
        progress(pos);
    }
    let mut end = vec![END];
    end.extend_from_slice(&hasher.finalize());
    write_sealed(stream, key, &end).await?;
    stream.flush().await?;
    let done: Done = read_json(stream, key).await?;
    if done.ok {
        Ok(())
    } else if done.reason.as_deref() == Some("hash") {
        Err(SendError::Corrupt)
    } else {
        Err(SendError::Rejected(done.reason.unwrap_or_default()))
    }
}

/// 接收端讀 Offer（呼叫端已經讀掉 [`MAGIC`]）。解不開、太舊或檔名不安全都回傳錯誤。
pub async fn read_offer<R: AsyncRead + Unpin>(r: &mut R, key: &Key) -> io::Result<(Offer, PathBuf)> {
    let offer: Offer = read_json(r, key).await?;
    let fresh = now_ms().abs_diff(offer.at) <= MAX_AGE.as_millis() as u64;
    let rel = safe_relative(&offer.name).filter(|_| fresh && valid_id(&offer.id) && offer.v == 1);
    rel.map(|rel| (offer, rel)).ok_or_else(|| io::ErrorKind::InvalidData.into())
}

/// 拒絕這次傳送。
pub async fn reject<W: AsyncWrite + Unpin>(w: &mut W, key: &Key, reason: &str) -> io::Result<()> {
    write_json(w, key, &Reply { offset: 0, reject: Some(reason.into()) }).await
}

/// 接收檔案內容到 `partial`（已經有一部分就接著收）。成功時檔案完整、雜湊相符。
pub async fn receive<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    key: &Key,
    offer: &Offer,
    partial: &Path,
    mut progress: impl FnMut(u64),
) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).read(true).write(true).truncate(false).open(partial).await?;
    let mut have = file.metadata().await?.len();
    if have > offer.size {
        file.set_len(0).await?;
        have = 0;
    }
    // 已經收到的部分先算進雜湊
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    file.seek(io::SeekFrom::Start(0)).await?;
    let mut pos = 0u64;
    while pos < have {
        let want = ((have - pos) as usize).min(CHUNK);
        file.read_exact(&mut buf[..want]).await?;
        hasher.update(&buf[..want]);
        pos += want as u64;
    }
    write_json(stream, key, &Reply { offset: have, reject: None }).await?;
    progress(pos);
    loop {
        let frame = read_sealed(stream, key, MAX_DATA).await?;
        match frame.first() {
            Some(&DATA) if frame.len() > 9 => {
                let at = u64::from_be_bytes(frame[1..9].try_into().unwrap_or_default());
                let data = &frame[9..];
                if at != pos || pos + data.len() as u64 > offer.size {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                file.write_all(data).await?;
                hasher.update(data);
                pos += data.len() as u64;
                progress(pos);
            }
            Some(&END) if frame.len() == 33 => {
                file.flush().await?;
                file.sync_all().await?;
                let ok = pos == offer.size && hasher.finalize()[..] == frame[1..];
                if !ok {
                    drop(file);
                    let _ = tokio::fs::remove_file(partial).await;
                }
                let reason = (!ok).then(|| "hash".to_string());
                write_json(stream, key, &Done { ok, reason }).await?;
                stream.flush().await?;
                return if ok { Ok(()) } else { Err(io::ErrorKind::InvalidData.into()) };
            }
            _ => return Err(io::ErrorKind::InvalidData.into()),
        }
    }
}

/// 接收資料夾裡不和現有檔案撞名的路徑：`報告.pdf` → `報告 (1).pdf`。
pub fn unique_path(dir: &Path, rel: &Path) -> PathBuf {
    let first = dir.join(rel);
    if !first.exists() {
        return first;
    }
    let parent = first.parent().map(Path::to_path_buf).unwrap_or_else(|| dir.to_path_buf());
    let stem = first.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = first.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..)
        .map(|n| parent.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("copyc-test-{tag}-{}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn unsafe_names_are_cleaned_or_refused() {
        assert_eq!(safe_relative("報告.pdf").unwrap(), PathBuf::from("報告.pdf"));
        assert_eq!(safe_relative("相片/2026/IMG 1.jpg").unwrap(), PathBuf::from("相片").join("2026").join("IMG 1.jpg"));
        for bad in ["", "/", "../secret", "a/../../b", "./x", "a//b", "..", "a/./b"] {
            assert!(safe_relative(bad).is_none(), "{bad:?}");
        }
        // 絕對路徑、磁碟代號與反斜線都變成一般字元，不會跳出接收資料夾
        assert_eq!(safe_relative(r"C:\Windows\x.dll").unwrap(), PathBuf::from("C__Windows_x.dll"));
        assert_eq!(safe_relative("CON.txt").unwrap(), PathBuf::from("_CON.txt"));
        assert_eq!(safe_relative("name. ").unwrap(), PathBuf::from("name"));
    }

    #[test]
    fn transfer_ids_are_stable_and_hex() {
        let a = transfer_id("dev", Path::new("/x/a.bin"), 10, 1);
        assert_eq!(a, transfer_id("dev", Path::new("/x/a.bin"), 10, 1));
        assert_ne!(a, transfer_id("dev", Path::new("/x/a.bin"), 11, 1));
        assert!(valid_id(&a));
    }

    #[test]
    fn unique_names_do_not_overwrite() {
        let dir = temp_dir("unique");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::write(dir.join("a (1).txt"), b"x").unwrap();
        assert_eq!(unique_path(&dir, Path::new("a.txt")), dir.join("a (2).txt"));
        assert_eq!(unique_path(&dir, Path::new("b.txt")), dir.join("b.txt"));
        let _ = std::fs::remove_dir_all(dir);
    }

    async fn round_trip(size: usize, already: usize) -> (Vec<u8>, Vec<u8>, Vec<u64>) {
        let dir = temp_dir("rt");
        let data: Vec<u8> = (0..size).map(|i| (i * 31 % 251) as u8).collect();
        let src = dir.join("src.bin");
        std::fs::write(&src, &data).unwrap();
        let partial = dir.join("x.part");
        std::fs::write(&partial, &data[..already]).unwrap();
        let key = Key::generate();
        let offer = Offer::new("0123456789abcdef0123".into(), "影片.mp4".into(), size as u64, "PC".into(), "id".into());
        let (mut a, mut b) = duplex(4 << 20);
        let (k1, k2, o1) = (key.clone(), key.clone(), offer.clone());
        let p2 = partial.clone();
        let recv = tokio::spawn(async move {
            let (got, rel) = read_offer(&mut b, &k2).await.unwrap();
            assert_eq!(rel, PathBuf::from("影片.mp4"));
            let mut seen = Vec::new();
            receive(&mut b, &k2, &got, &p2, |n| seen.push(n)).await.unwrap();
            seen
        });
        let mut sent = Vec::new();
        send(&mut a, &k1, &o1, &src, |n| sent.push(n)).await.unwrap();
        let seen = recv.await.unwrap();
        assert_eq!(*sent.first().unwrap(), already as u64, "從對方已有的地方接著傳");
        assert_eq!(*seen.last().unwrap(), size as u64);
        let got = std::fs::read(&partial).unwrap();
        let _ = std::fs::remove_dir_all(dir);
        (data, got, sent)
    }

    #[tokio::test]
    async fn sends_whole_file_in_encrypted_chunks() {
        let (data, got, sent) = round_trip(CHUNK * 2 + 12345, 0).await;
        assert_eq!(data, got);
        assert_eq!(sent.len(), 4); // 0、兩個整塊、最後一塊
    }

    #[tokio::test]
    async fn resumes_from_what_the_receiver_already_has() {
        let (data, got, sent) = round_trip(CHUNK * 3, CHUNK + 7).await;
        assert_eq!(data, got);
        assert_eq!(sent[0], (CHUNK + 7) as u64);
    }

    #[tokio::test]
    async fn empty_files_work() {
        let (data, got, _) = round_trip(0, 0).await;
        assert_eq!(data, got);
    }

    #[tokio::test]
    async fn wrong_key_and_stale_offers_are_refused() {
        let key = Key::generate();
        let (mut a, mut b) = duplex(1 << 16);
        let offer = Offer::new("0123456789abcdef".into(), "a.txt".into(), 1, "PC".into(), "id".into());
        write_json(&mut a, &Key::generate(), &offer).await.unwrap();
        assert!(read_offer(&mut b, &key).await.is_err());
        let mut old = offer.clone();
        old.at -= MAX_AGE.as_millis() as u64 + 60_000;
        write_json(&mut a, &key, &old).await.unwrap();
        assert!(read_offer(&mut b, &key).await.is_err());
        let mut evil = offer;
        evil.name = "../../.ssh/authorized_keys".into();
        write_json(&mut a, &key, &evil).await.unwrap();
        assert!(read_offer(&mut b, &key).await.is_err());
    }

    #[tokio::test]
    async fn corrupted_partial_is_detected_and_discarded() {
        let dir = temp_dir("bad");
        let src = dir.join("src.bin");
        std::fs::write(&src, vec![7u8; 5000]).unwrap();
        let partial = dir.join("x.part");
        // 接收端手上的前半段被改過：最後的雜湊會不符，傳送端要知道該重傳
        std::fs::write(&partial, vec![9u8; 2000]).unwrap();
        let key = Key::generate();
        let offer = Offer::new("0123456789abcdef".into(), "a.bin".into(), 5000, "PC".into(), "id".into());
        let (mut a, mut b) = duplex(1 << 16);
        let (k2, p2) = (key.clone(), partial.clone());
        let recv = tokio::spawn(async move {
            let (got, _) = read_offer(&mut b, &k2).await.unwrap();
            receive(&mut b, &k2, &got, &p2, |_| {}).await
        });
        assert!(matches!(send(&mut a, &key, &offer, &src, |_| {}).await, Err(SendError::Corrupt)));
        assert!(recv.await.unwrap().is_err());
        assert!(!partial.exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
