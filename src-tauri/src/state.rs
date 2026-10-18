//! 全域狀態：設定、通知框狀態機、微歷史與統計。

use crate::{classify::Payload, hud::HudState, settings::Settings};
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const HISTORY_CAP: usize = 20;
const RECENT_APPS_CAP: usize = 12;

#[derive(Debug, Clone)]
pub struct HistoryItem {
    pub payload: Payload,
    pub source: Option<String>,
    pub at: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryOut {
    pub payload: Payload,
    pub source: Option<String>,
    /// Unix 毫秒
    pub at: u64,
}

#[derive(Debug, Default, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    /// 本次啟動以來的複製次數（同一內容連按只算一次）
    pub copies: u64,
    /// 連按確認次數（「已確認 x3 次」中多出來的那幾下）
    pub confirms: u64,
}

pub struct AppState {
    pub settings: Mutex<Settings>,
    pub settings_path: PathBuf,
    pub hud: Mutex<HudState>,
    pub history: Mutex<VecDeque<HistoryItem>>,
    pub stats: Mutex<Stats>,
    pub recent_apps: Mutex<Vec<String>>,
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl AppState {
    pub fn new(settings: Settings, settings_path: PathBuf) -> Self {
        Self {
            settings: Mutex::new(settings),
            settings_path,
            hud: Mutex::new(HudState::default()),
            history: Mutex::new(VecDeque::with_capacity(HISTORY_CAP)),
            stats: Mutex::new(Stats::default()),
            recent_apps: Mutex::new(Vec::new()),
        }
    }

    /// 記錄一次複製。連按同一內容只更新時間，不新增紀錄。
    pub fn record(&self, payload: &Payload, source: Option<&str>, repeat: bool) {
        let at = now_ms();
        {
            let mut history = self.history.lock();
            match history.front_mut() {
                Some(front) if repeat => front.at = at,
                _ => {
                    history.push_front(HistoryItem { payload: payload.clone(), source: source.map(Into::into), at });
                    history.truncate(HISTORY_CAP);
                }
            }
        }
        {
            let mut stats = self.stats.lock();
            if repeat {
                stats.confirms += 1;
            } else {
                stats.copies += 1;
            }
        }
        if let Some(app) = source.filter(|s| !s.eq_ignore_ascii_case("copyc")) {
            let mut apps = self.recent_apps.lock();
            apps.retain(|a| !a.eq_ignore_ascii_case(app));
            apps.insert(0, app.to_string());
            apps.truncate(RECENT_APPS_CAP);
        }
    }

    /// 對外輸出歷史；隱私打碼模式開啟時，連先前的紀錄也一併遮蔽。
    pub fn history_out(&self, skip: usize, take: usize) -> Vec<HistoryOut> {
        let private = self.settings.lock().privacy_mode;
        self.history
            .lock()
            .iter()
            .skip(skip)
            .take(take)
            .map(|h| HistoryOut {
                payload: if private { Payload::Private } else { h.payload.clone() },
                source: if private { None } else { h.source.clone() },
                at: h.at,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(p: &str) -> Payload {
        Payload::Text { preview: p.into(), truncated: false, chars: p.len(), lines: 1 }
    }

    #[test]
    fn repeat_updates_instead_of_appending() {
        let s = AppState::new(Settings::default(), PathBuf::new());
        s.record(&text("a"), Some("Notes"), false);
        s.record(&text("a"), Some("Notes"), true);
        s.record(&text("a"), Some("Notes"), true);
        assert_eq!(s.history.lock().len(), 1);
        let stats = *s.stats.lock();
        assert_eq!((stats.copies, stats.confirms), (1, 2));
    }

    #[test]
    fn privacy_mode_masks_existing_history() {
        let s = AppState::new(Settings::default(), PathBuf::new());
        s.record(&text("meeting notes"), Some("Notes"), false);
        s.settings.lock().privacy_mode = true;
        let out = s.history_out(0, 3);
        assert_eq!(out[0].payload, Payload::Private);
        assert!(out[0].source.is_none());
    }

    #[test]
    fn recent_apps_are_unique_and_newest_first() {
        let s = AppState::new(Settings::default(), PathBuf::new());
        for app in ["Notes", "Safari", "Notes"] {
            s.record(&text(app), Some(app), false);
        }
        assert_eq!(*s.recent_apps.lock(), vec!["Notes".to_string(), "Safari".to_string()]);
    }
}
