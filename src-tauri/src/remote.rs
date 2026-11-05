//! 遠端資料：不用發新版本（也不經過 Microsoft Store）就能調整的內容。
//!
//! App 啟動時與之後每 6 小時，從官網下載一份經過簽章的資料檔（`config/v1.json`），
//! 用內建的公鑰驗證通過才採用，並存一份在本機，離線時沿用上一次的內容。資料檔只有資料、沒有程式碼：
//!
//! - `notices`：顯示在偏好設定上方的公告（例如服務異常、重要提醒），可限定平台、版本與到期時間。
//! - `blacklist`：額外當作密碼管理器處理的 App 名稱（遮蔽內容、不寫進歷史）。
//! - `strings`：修正介面文字或翻譯（只取代現有的文字，不能加入 HTML）。
//! - `features`：緊急關閉即時同步、區域網路直傳或歷史備份（例如中繼伺服器出問題時）。
//!
//! 發佈方式見 `config/README.md`：修改 `config/remote.json` 推上 GitHub，Actions 會用私鑰簽章後放到官網。

use crate::state::now_ms;
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, VerifyingKey};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tauri::{AppHandle, Emitter, Manager};

pub const URL: &str = "https://www.copyc.app.dreamsworld.top/config/v1.json";
/// 驗證資料檔的公鑰（Ed25519）；對應的私鑰只在 GitHub Secrets 的 COPYC_REMOTE_SIGNING_KEY。
const PUBLIC_KEY: &str = "kAzHQDNaCADrcbELlqkZs+AjVV9cBJ2c/rXjicCx3Qo=";
const CACHE_FILE: &str = "remote.json";
const FIRST_FETCH: Duration = Duration::from_secs(2);
const REFRESH: Duration = Duration::from_secs(6 * 60 * 60);
/// 失敗（離線）時隔這麼久再試，直到成功一次。
const RETRY: Duration = Duration::from_secs(10 * 60);
const MAX_BYTES: usize = 256 * 1024;

/// 線路上的檔案：資料本身（base64）與它的簽章。簽的是解碼後的原始位元組，不受空白或排版影響。
#[derive(Deserialize)]
struct Envelope {
    data: String,
    sig: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Remote {
    /// 發佈時間（秒），只採用比手上更新的資料，避免被換回舊版本
    pub seq: u64,
    pub notices: Vec<Notice>,
    pub blacklist: Vec<String>,
    /// 語言 → 文字鍵 → 新的文字
    pub strings: HashMap<String, HashMap<String, String>>,
    pub features: Features,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Features {
    pub live: bool,
    pub lan: bool,
    pub backup: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self { live: true, lan: true, backup: true }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Notice {
    pub id: String,
    /// "info" 或 "warn"
    pub level: String,
    /// 語言（zh-Hant、zh-Hans、en、ja、ko、fr、ms）→ 文字；沒有目前語言時用 "en"
    pub title: HashMap<String, String>,
    pub body: HashMap<String, String>,
    /// 「了解更多」連結（只接受 https）
    pub url: Option<String>,
    /// 到期時間（毫秒），之後不再顯示
    pub until: Option<u64>,
    /// "windows" / "macos"；空的代表全部
    pub platforms: Vec<String>,
    /// "web"（官網版）/ "store"（Microsoft Store 版）；空的代表全部
    pub channels: Vec<String>,
    /// 只顯示給這個範圍內的版本（含）
    pub min_version: Option<String>,
    pub max_version: Option<String>,
}

/// 給偏好設定頁的公告（已選好語言）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NoticeOut {
    pub id: String,
    pub level: String,
    pub title: String,
    pub body: String,
    pub has_url: bool,
}

static CURRENT: RwLock<Option<Arc<Remote>>> = RwLock::new(None);

fn current() -> Option<Arc<Remote>> {
    CURRENT.read().clone()
}

/// 驗證簽章並解析；任何一步不對都回傳 None。
fn verify(raw: &[u8], public_key: &[u8; 32]) -> Option<Remote> {
    if raw.len() > MAX_BYTES {
        return None;
    }
    let env: Envelope = serde_json::from_slice(raw).ok()?;
    let data = STANDARD.decode(env.data.trim()).ok()?;
    let sig: [u8; 64] = STANDARD.decode(env.sig.trim()).ok()?.try_into().ok()?;
    let key = VerifyingKey::from_bytes(public_key).ok()?;
    key.verify_strict(&data, &Signature::from_bytes(&sig)).ok()?;
    serde_json::from_slice(&data).ok()
}

fn public_key() -> [u8; 32] {
    STANDARD.decode(PUBLIC_KEY).ok().and_then(|k| k.try_into().ok()).expect("valid built-in public key")
}

fn cache_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join(CACHE_FILE))
}

/// 啟動時（建立同步之前）讀取上一次下載的資料，離線也能沿用。
pub fn load_cached(app: &AppHandle) {
    let Some(raw) = cache_path(app).and_then(|p| std::fs::read(p).ok()) else { return };
    if let Some(remote) = verify(&raw, &public_key()) {
        *CURRENT.write() = Some(Arc::new(remote));
    }
}

/// 背景工作：下載、驗證、比手上新就採用並存檔。
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("CopyC/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        tokio::time::sleep(FIRST_FETCH).await;
        loop {
            let wait = match fetch(&http).await {
                Some(raw) => {
                    apply(&app, &raw);
                    REFRESH
                }
                None => RETRY,
            };
            tokio::time::sleep(wait).await;
        }
    });
}

async fn fetch(http: &reqwest::Client) -> Option<Vec<u8>> {
    let resp = http.get(URL).header("Cache-Control", "no-cache").send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    (bytes.len() <= MAX_BYTES).then(|| bytes.to_vec())
}

fn apply(app: &AppHandle, raw: &[u8]) {
    let Some(remote) = verify(raw, &public_key()) else { return };
    if current().is_some_and(|c| c.seq >= remote.seq) {
        return;
    }
    if let Some(path) = cache_path(app) {
        let _ = std::fs::write(path, raw);
    }
    *CURRENT.write() = Some(Arc::new(remote));
    if let Some(cloud) = app.try_state::<crate::cloud::Cloud>() {
        cloud.apply_remote(&app.state::<crate::state::AppState>().settings.lock());
    }
    let _ = app.emit("remote:changed", ());
}

/* ---------------- 給其他模組用 ---------------- */

/// 同步功能的緊急開關（沒有資料時全部開啟）。
pub fn features() -> Features {
    current().map(|r| r.features).unwrap_or_default()
}

/// 遠端名單裡的 App（比對方式和使用者的黑名單相同）。
pub fn blacklisted(hay: &[String]) -> bool {
    current().is_some_and(|r| {
        r.blacklist.iter().any(|entry| {
            let needle = entry.trim().to_lowercase();
            !needle.is_empty() && hay.iter().any(|h| !h.is_empty() && (h == &needle || h.contains(&needle)))
        })
    })
}

/// 某個語言要取代的介面文字。
pub fn strings(lang: &str) -> HashMap<String, String> {
    current().and_then(|r| r.strings.get(lang).cloned()).unwrap_or_default()
}

/// 現在要顯示的公告。
pub fn notices(lang: &str, platform: &str, packaged: bool, version: &str) -> Vec<NoticeOut> {
    let Some(r) = current() else { return Vec::new() };
    let now = now_ms();
    let channel = if packaged { "store" } else { "web" };
    r.notices
        .iter()
        .filter(|n| !n.id.is_empty() && n.until.map_or(true, |until| until > now))
        .filter(|n| n.platforms.is_empty() || n.platforms.iter().any(|p| p == platform))
        .filter(|n| n.channels.is_empty() || n.channels.iter().any(|c| c == channel))
        .filter(|n| n.min_version.as_deref().map_or(true, |min| parse_version(min) <= parse_version(version)))
        .filter(|n| n.max_version.as_deref().map_or(true, |max| parse_version(max) >= parse_version(version)))
        .filter_map(|n| {
            let title = pick(&n.title, lang)?;
            Some(NoticeOut {
                id: n.id.clone(),
                level: if n.level == "warn" { "warn".into() } else { "info".into() },
                title,
                body: pick(&n.body, lang).unwrap_or_default(),
                has_url: n.url.as_deref().is_some_and(is_https),
            })
        })
        .collect()
}

/// 公告的連結（依 id 找，頁面不能指定任意網址）。
pub fn notice_url(id: &str) -> Option<String> {
    current()?.notices.iter().find(|n| n.id == id)?.url.clone().filter(|u| is_https(u))
}

fn is_https(url: &str) -> bool {
    url.starts_with("https://") && url.len() <= 500 && !url.chars().any(char::is_whitespace)
}

fn pick(map: &HashMap<String, String>, lang: &str) -> Option<String> {
    map.get(lang).or_else(|| map.get("en")).or_else(|| map.values().next()).filter(|s| !s.trim().is_empty()).cloned()
}

/* ---------------- 小工具 ---------------- */

pub fn parse_version(v: &str) -> [u64; 3] {
    let mut out = [0; 3];
    for (slot, part) in out.iter_mut().zip(v.trim().trim_start_matches('v').split('.')) {
        *slot = part.parse().unwrap_or(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn sign(key: &SigningKey, remote: &serde_json::Value) -> Vec<u8> {
        let data = serde_json::to_vec(remote).unwrap();
        let sig = key.sign(&data).to_bytes();
        serde_json::json!({ "data": STANDARD.encode(&data), "sig": STANDARD.encode(sig) }).to_string().into_bytes()
    }

    #[test]
    fn only_correctly_signed_data_is_accepted() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let public = key.verifying_key().to_bytes();
        let raw = sign(&key, &serde_json::json!({ "seq": 5, "blacklist": ["Proton Pass"] }));
        let remote = verify(&raw, &public).unwrap();
        assert_eq!((remote.seq, remote.blacklist.as_slice()), (5, &["Proton Pass".to_string()][..]));
        assert!(remote.features.live && remote.features.lan && remote.features.backup);

        // 別把鑰匙簽的、或內容被改過，都不採用
        let other = SigningKey::from_bytes(&[8; 32]);
        assert!(verify(&sign(&other, &serde_json::json!({ "seq": 5 })), &public).is_none());
        let mut env: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        env["data"] = STANDARD.encode(br#"{"seq":6,"blacklist":[]}"#).into();
        assert!(verify(env.to_string().as_bytes(), &public).is_none());
        assert!(verify(b"not json", &public).is_none());
    }

    #[test]
    fn signature_made_by_the_publish_script_verifies() {
        // scripts/sign-remote.mjs 用 Node.js 以測試金鑰簽出的檔案（確認兩邊格式一致）
        let public: [u8; 32] =
            STANDARD.decode("1c9citBP3h6T0zmkaq5DDT8MY7lPJUP2G2JoNx1yNXM=").unwrap().try_into().unwrap();
        let raw = include_bytes!("../tests/remote-fixture.json");
        let remote = verify(raw, &public).unwrap();
        assert!(remote.seq > 0);
        assert_eq!(remote.notices[0].id, "fixture");
        assert!(!remote.features.lan);
    }

    #[test]
    fn built_in_public_key_is_valid() {
        assert!(VerifyingKey::from_bytes(&public_key()).is_ok());
    }

    #[test]
    fn notices_are_filtered_and_localized() {
        let notice = |id: &str| Notice {
            id: id.into(),
            title: HashMap::from([("en".into(), "Hello".into()), ("zh-Hant".into(), "你好".into())]),
            ..Default::default()
        };
        let remote = Remote {
            seq: 1,
            notices: vec![
                Notice { url: Some("https://example.com/x".into()), ..notice("all") },
                Notice { until: Some(1), ..notice("expired") },
                Notice { platforms: vec!["macos".into()], ..notice("mac") },
                Notice { channels: vec!["store".into()], ..notice("store") },
                Notice { min_version: Some("1.2.0".into()), ..notice("future") },
                Notice { max_version: Some("1.1.0".into()), ..notice("old") },
                Notice { url: Some("http://insecure".into()), ..notice("http") },
            ],
            ..Default::default()
        };
        *CURRENT.write() = Some(Arc::new(remote));
        let ids = |lang, platform, packaged, version| {
            notices(lang, platform, packaged, version).into_iter().map(|n| n.id).collect::<Vec<_>>()
        };
        assert_eq!(ids("zh-Hant", "windows", false, "1.1.1"), ["all", "http"]);
        assert_eq!(ids("en", "macos", true, "1.0.9"), ["all", "mac", "store", "old", "http"]);
        let zh = notices("zh-Hant", "windows", false, "1.1.1");
        assert_eq!(zh[0].title, "你好");
        assert!(zh[0].has_url && !zh[1].has_url);
        assert_eq!(notices("ja", "windows", false, "1.1.1")[0].title, "Hello");
        assert_eq!(notice_url("all").as_deref(), Some("https://example.com/x"));
        assert_eq!(notice_url("http"), None);
        *CURRENT.write() = None;
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(parse_version("1.10.0") > parse_version("1.9.9"));
        assert_eq!(parse_version("v1.1"), [1, 1, 0]);
    }
}
