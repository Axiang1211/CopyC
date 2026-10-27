//! 歷史備份（B）：每台裝置把自己的文字類紀錄依 UTC 日寫成一個加密檔
//! `h1-<裝置>-<日>`，刪除紀錄寫在 `t1-<裝置>`。其他裝置下載比上次新的檔案並合併。

use super::{drive::Drive, Cloud, CloudError};
use crate::{
    classify,
    history::{History, DAY_MS},
    state::{now_ms, AppState},
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// 雲端保留的天數；更舊的備份檔與刪除紀錄由各裝置自己清掉。
const KEEP_DAYS: u64 = 90;
/// 沒有新紀錄時多久檢查一次其他裝置。
const INTERVAL: Duration = Duration::from_secs(300);
/// 有新紀錄後等一下再上傳，把連續幾次複製合併成一次。
const DEBOUNCE: Duration = Duration::from_secs(15);

#[derive(Serialize, Deserialize)]
struct DayFile {
    v: u32,
    device: String,
    entries: Vec<Record>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    at: u64,
    text: String,
}

#[derive(Serialize, Deserialize)]
struct TombFile {
    v: u32,
    hashes: Vec<u64>,
}

pub async fn run(app: AppHandle) {
    let cloud = app.state::<Cloud>();
    let mut config = cloud.config.subscribe();
    loop {
        let c = *config.borrow_and_update();
        if !(c.signed_in && c.backup) {
            if config.changed().await.is_err() {
                return;
            }
            continue;
        }
        cloud.update(|rt| rt.syncing = true);
        let result = once(&app, &cloud).await;
        cloud.update(|rt| {
            rt.syncing = false;
            // 登入失效由 forget() 處理；其他錯誤顯示在設定頁，下次成功就清掉
            if rt.error != Some(CloudError::SignedOut) {
                rt.error = result.err();
            }
        });
        let wait = if result.is_err() { INTERVAL / 2 } else { INTERVAL };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = cloud.wake.notified() => tokio::time::sleep(DEBOUNCE).await,
            _ = cloud.sync_now.notified() => {}
            r = config.changed() => if r.is_err() { return },
        }
    }
}

async fn once(app: &AppHandle, cloud: &Cloud) -> Result<(), CloudError> {
    let state = app.state::<AppState>();
    let Some(store) = state.store.get() else { return Ok(()) };
    let token = cloud.access_token().await?;
    let drive = Drive { http: &cloud.http, token: &token };
    let files = drive.list().await?;
    let key = cloud.load_key(&drive, &files).await?;
    let me = &cloud.device_id;
    let now = now_ms();

    // 1. 上傳：有變動的日子整天重寫（刪除的紀錄也就從備份裡消失了）
    for day in store.dirty_days().map_err(db)? {
        let rows = store.day_texts(day).map_err(db)?;
        let name = format!("h1-{me}-{day}");
        let existing = files.iter().find(|f| f.name == name);
        if rows.is_empty() {
            if let Some(f) = existing {
                drive.delete(&f.id).await?;
            }
        } else {
            let body = DayFile {
                v: 1,
                device: cloud.device_name.clone(),
                entries: rows.iter().map(|(_, at, text)| Record { at: *at, text: text.clone() }).collect(),
            };
            let data = key.seal(&serde_json::to_vec(&body).unwrap_or_default());
            match existing {
                Some(f) => drive.update(&f.id, data).await?,
                None => drop(drive.create(&name, data).await?),
            }
        }
        let ids: Vec<i64> = rows.iter().map(|(id, _, _)| *id).collect();
        store.mark_day_synced(day, &ids).map_err(db)?;
    }
    if store.has_unpushed_tombstones().map_err(db)? {
        let hashes = store.tombstones_since(now.saturating_sub(KEEP_DAYS * DAY_MS)).map_err(db)?;
        let data = key.seal(&serde_json::to_vec(&TombFile { v: 1, hashes }).unwrap_or_default());
        let name = format!("t1-{me}");
        match files.iter().find(|f| f.name == name) {
            Some(f) => drive.update(&f.id, data).await?,
            None => drop(drive.create(&name, data).await?),
        }
        store.mark_tombstones_pushed().map_err(db)?;
    }

    // 2. 下載其他裝置比上次新的檔案：先套用刪除，再合併紀錄（避免把刪掉的又加回來）
    let last_pull = cloud.account.lock().as_ref().map(|a| a.last_pull.clone()).unwrap_or_default();
    let mut newer: Vec<_> = files
        .iter()
        .filter(|f| f.modified_time > last_pull && !f.name.contains(me.as_str()))
        .filter(|f| f.name.starts_with("t1-") || f.name.starts_with("h1-"))
        .collect();
    newer.sort_by_key(|f| (!f.name.starts_with("t1-"), f.modified_time.clone()));
    let mut changed = false;
    let mut newest = last_pull;
    for f in newer {
        let Some(plain) = key.open(&drive.download(&f.id).await?) else {
            // 用舊金鑰加密的檔案（對方還沒換到新金鑰），等它重新上傳
            continue;
        };
        if f.name.starts_with("t1-") {
            if let Ok(t) = serde_json::from_slice::<TombFile>(&plain) {
                changed |= store.apply_tombstones(&t.hashes).map_err(db)?;
            }
        } else if let Ok(day) = serde_json::from_slice::<DayFile>(&plain) {
            changed |= merge(store, &day)?;
        }
        newest = newest.max(f.modified_time.clone());
    }
    if changed {
        let limit = state.settings.lock().history_limit;
        store.trim(limit).map_err(db)?;
        let _ = app.emit("history:changed", ());
    }

    // 3. 清掉自己超過保留期的備份檔
    let oldest = now / DAY_MS - KEEP_DAYS;
    let prefix = format!("h1-{me}-");
    for f in &files {
        let old = f.name.strip_prefix(&prefix).and_then(|d| d.parse::<u64>().ok()).is_some_and(|d| d < oldest);
        if old {
            drive.delete(&f.id).await?;
        }
    }

    if let Some(a) = cloud.account.lock().as_mut() {
        a.last_pull = newest;
        a.last_sync = Some(now);
    }
    cloud.save_account();
    Ok(())
}

fn merge(store: &History, day: &DayFile) -> Result<bool, CloudError> {
    let mut changed = false;
    for r in &day.entries {
        if let Some(entry) = classify::remote_entry(&r.text) {
            changed |= store.add_remote(&entry, classify::text_hash(&r.text), r.at, &day.device).map_err(db)?;
        }
    }
    Ok(changed)
}

fn db(e: rusqlite::Error) -> CloudError {
    eprintln!("copyc: 同步時讀寫歷史資料庫失敗：{e}");
    CloudError::Network
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::Category;

    fn local(h: &History, text: &str, at: u64) {
        let e = classify::remote_entry(text).unwrap();
        h.add(&e, None, classify::text_hash(text), at, 1000).unwrap();
    }

    /// 兩台裝置透過「備份檔」互相合併，刪除也會傳過去。
    #[test]
    fn two_devices_merge_and_propagate_deletes() {
        let a = History::in_memory();
        let b = History::in_memory();
        local(&a, "from A", DAY_MS * 20_000 + 5);
        local(&a, "https://example.com", DAY_MS * 20_000 + 9);
        local(&b, "from B", DAY_MS * 20_001);

        // A 上傳 → B 合併
        let days = a.dirty_days().unwrap();
        assert_eq!(days, [20_000]);
        let rows = a.day_texts(20_000).unwrap();
        let file = DayFile {
            v: 1,
            device: "A-PC".into(),
            entries: rows.iter().map(|(_, at, t)| Record { at: *at, text: t.clone() }).collect(),
        };
        a.mark_day_synced(20_000, &rows.iter().map(|r| r.0).collect::<Vec<_>>()).unwrap();
        assert!(a.dirty_days().unwrap().is_empty());
        assert!(merge(&b, &file).unwrap());
        assert!(!merge(&b, &file).unwrap(), "same file twice changes nothing");
        let page = b.query("", None, 0, 10).unwrap();
        assert_eq!(page.total, 3);
        let link = page.items.iter().find(|i| i.category == "link").unwrap();
        assert_eq!(link.origin.as_deref(), Some("A-PC"));
        // 其他裝置的紀錄不會被 B 再上傳
        assert_eq!(b.dirty_days().unwrap(), [20_001]);

        // B 刪掉 A 的連結 → 墓碑 → A 套用後刪除並重寫那一天
        b.delete(link.id).unwrap();
        assert!(b.has_unpushed_tombstones().unwrap());
        let hashes = b.tombstones_since(0).unwrap();
        assert!(a.apply_tombstones(&hashes).unwrap());
        assert_eq!(a.query("", Some("link"), 0, 10).unwrap().total, 0);
        assert_eq!(a.dirty_days().unwrap(), [20_000]);
        assert_eq!(a.day_texts(20_000).unwrap().len(), 1);
        // 再次合併舊的備份檔也不會把刪掉的加回來
        merge(&b, &file).unwrap();
        assert_eq!(b.query("", Some("link"), 0, 10).unwrap().total, 0);

        // 在 B 重新複製同一個連結：變回本機紀錄、墓碑作廢
        local(&b, "https://example.com", DAY_MS * 20_002);
        let page = b.query("", Some("link"), 0, 10).unwrap();
        assert_eq!(page.items[0].origin, None);
    }

    #[test]
    fn secrets_and_files_never_sync() {
        let h = History::in_memory();
        let secret = crate::classify::Entry {
            category: Category::Secret,
            payload: crate::classify::Payload::Generic { label: "x".into() },
            text: Some("Tr0ub4dor&3x!".into()),
            paths: Vec::new(),
        };
        h.add(&secret, None, 1, DAY_MS, 100).unwrap();
        let files = crate::classify::Entry {
            category: Category::Files,
            payload: crate::classify::Payload::Generic { label: "files".into() },
            text: None,
            paths: vec!["/tmp/a.pdf".into()],
        };
        h.add(&files, None, 2, DAY_MS, 100).unwrap();
        assert!(h.dirty_days().unwrap().is_empty());
        // 遠端傳來看起來像密碼的內容也不收
        assert!(classify::remote_entry("Tr0ub4dor&3x!Zq9#").is_none());
    }

    #[test]
    fn sign_out_removes_other_devices_entries() {
        let h = History::in_memory();
        local(&h, "mine", 1);
        let e = classify::remote_entry("theirs").unwrap();
        h.add_remote(&e, classify::text_hash("theirs"), 2, "Other").unwrap();
        h.delete_remote().unwrap();
        let page = h.query("", None, 0, 10).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].text.as_deref(), Some("mine"));
    }
}
