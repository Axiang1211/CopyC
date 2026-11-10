//! 歷史紀錄（Android 版，唯讀）：下載電腦版備份到 Google Drive 的每日加密檔（h1-裝置-日），
//! 解密後合併成一份清單；各電腦刪除的紀錄（t1-裝置）也跟著拿掉。結果存在手機上，離線也看得到。
//! 檔案格式與電腦版 src-tauri/src/cloud/sync.rs 相同。

use crate::cloud::{drive::Drive, now_ms, texthash::text_hash, Cloud, CloudError};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const CACHE_FILE: &str = "history.json";
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// 和電腦版雲端保留的天數相同。
const KEEP_DAYS: u64 = 90;
const MAX_ITEMS: usize = 3000;

#[derive(Deserialize)]
struct DayFile {
    device: String,
    entries: Vec<Record>,
}

#[derive(Deserialize)]
struct Record {
    at: u64,
    text: String,
}

#[derive(Deserialize)]
struct TombFile {
    hashes: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub text: String,
    pub device: String,
    pub at: u64,
    /// 和電腦版相同的內容雜湊（用來套用刪除）；畫面不需要
    #[serde(skip_serializing_if = "is_zero", default)]
    pub hash: u64,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Cache {
    /// 下載過的最新檔案修改時間（RFC 3339），之後只下載比它新的
    last_pull: String,
    items: Vec<Item>,
    tombstones: Vec<u64>,
}

fn cache_path(cloud: &Cloud) -> std::path::PathBuf {
    cloud.dir().join(CACHE_FILE)
}

fn load(cloud: &Cloud) -> Cache {
    std::fs::read(cache_path(cloud)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// 手機上已經有的紀錄（不連網）。
pub fn cached(cloud: &Cloud) -> Vec<Item> {
    load(cloud).items
}

/// 下載比上次新的備份檔並合併，回傳完整清單（新的在前）。
pub async fn refresh(cloud: &Cloud) -> Result<Vec<Item>, CloudError> {
    let key = cloud.key().await?;
    let token = cloud.access_token().await?;
    let drive = Drive { http: &cloud.http, token: &token };
    let files = drive.list().await?;
    let mut cache = load(cloud);
    let mut newer: Vec<_> = files
        .iter()
        .filter(|f| f.modified_time > cache.last_pull)
        .filter(|f| f.name.starts_with("t1-") || f.name.starts_with("h1-"))
        .collect();
    // 先套用刪除，再合併紀錄（避免把刪掉的又加回來）
    newer.sort_by_key(|f| (!f.name.starts_with("t1-"), f.modified_time.clone()));
    let mut newest = cache.last_pull.clone();
    for f in newer {
        let Some(plain) = key.open(&drive.download(&f.id).await?) else { continue };
        if f.name.starts_with("t1-") {
            if let Ok(t) = serde_json::from_slice::<TombFile>(&plain) {
                cache.tombstones.extend(t.hashes);
            }
        } else if let Ok(day) = serde_json::from_slice::<DayFile>(&plain) {
            merge(&mut cache.items, &day);
        }
        newest = newest.max(f.modified_time.clone());
    }
    cache.last_pull = newest;
    tidy(&mut cache, now_ms());
    let _ = std::fs::write(cache_path(cloud), serde_json::to_vec(&cache).unwrap_or_default());
    Ok(cache.items)
}

fn merge(items: &mut Vec<Item>, day: &DayFile) {
    for r in &day.entries {
        if r.text.trim().is_empty() {
            continue;
        }
        items.push(Item { text: r.text.clone(), device: day.device.clone(), at: r.at, hash: text_hash(&r.text) });
    }
}

/// 拿掉已刪除、太舊與重複的紀錄（同一段文字只留最新一次），新的排前面。
fn tidy(cache: &mut Cache, now: u64) {
    let oldest = now.saturating_sub(KEEP_DAYS * DAY_MS);
    let dead: HashSet<u64> = cache.tombstones.iter().copied().collect();
    let mut newest: HashMap<u64, Item> = HashMap::new();
    for item in cache.items.drain(..) {
        if item.at < oldest || dead.contains(&item.hash) {
            continue;
        }
        match newest.get(&item.hash) {
            Some(existing) if existing.at >= item.at => {}
            _ => {
                newest.insert(item.hash, item);
            }
        }
    }
    let mut items: Vec<Item> = newest.into_values().collect();
    items.sort_by(|a, b| b.at.cmp(&a.at));
    items.truncate(MAX_ITEMS);
    cache.items = items;
    cache.tombstones.sort_unstable();
    cache.tombstones.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str, at: u64) -> Item {
        Item { text: text.into(), device: "PC".into(), at, hash: text_hash(text) }
    }

    #[test]
    fn merges_dedupes_and_applies_deletes() {
        let now = 100 * DAY_MS;
        let mut cache = Cache {
            items: vec![item("a", now - 10), item("a", now - 5), item("b", now - 3), item("old", 1)],
            tombstones: vec![text_hash("b")],
            ..Default::default()
        };
        tidy(&mut cache, now);
        let texts: Vec<_> = cache.items.iter().map(|i| (i.text.as_str(), i.at)).collect();
        assert_eq!(texts, [("a", now - 5)]);
    }

    #[test]
    fn reads_desktop_day_files() {
        let raw = r#"{"v":1,"device":"OFFICE-PC","entries":[{"at":5,"text":"482913"},{"at":6,"text":" "}]}"#;
        let day: DayFile = serde_json::from_str(raw).unwrap();
        let mut items = Vec::new();
        merge(&mut items, &day);
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].device.as_str(), items[0].hash), ("OFFICE-PC", text_hash("482913")));
    }
}
