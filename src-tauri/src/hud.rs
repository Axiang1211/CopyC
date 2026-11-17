//! 通知框（HUD）控制器：定位、不搶焦點的顯示 / 隱藏、連按防抖動與懸停歷史。
//!
//! 生命週期：Rust 顯示視窗並送出 `hud:show` → 前端播放進場動畫並計時 →
//! 淡出結束後呼叫 `hud_finished(seq)` → 若期間沒有新事件，Rust 才隱藏視窗。
//! 連按時只會送出新的 `hud:show`（count 遞增），視窗不會關閉再重開。

use crate::{
    classify::{Clip, Outcome, Payload},
    platform,
    settings::{FullscreenAction, Position, Settings},
    state::{AppState, HistoryOut},
};
use serde::Serialize;
use std::{
    thread,
    time::{Duration, Instant},
};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

pub const HUD_LABEL: &str = "hud";
/// 通知框視窗尺寸（邏輯像素）。視窗本身透明且滑鼠穿透，內容貼齊其中一角。
const HUD_W: f64 = 540.0;
const HUD_H: f64 = 300.0;
/// 與 hud.css 的 --stage-pad 一致：內容與視窗邊緣的距離（留給陰影）。
const STAGE_PAD: f64 = 24.0;
/// 規格：1 秒內連續複製同一內容視為連按。
const REPEAT_WINDOW: Duration = Duration::from_millis(1000);
const HOVER_POLL: Duration = Duration::from_millis(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VAlign {
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Anchor {
    pub h: HAlign,
    pub v: VAlign,
}

impl Default for Anchor {
    fn default() -> Self {
        Self { h: HAlign::Center, v: VAlign::Top }
    }
}

#[derive(Debug, Default)]
pub struct HudState {
    seq: u64,
    visible: bool,
    last_hash: Option<u64>,
    last_at: Option<Instant>,
    count: u32,
    anchor: Anchor,
    /// 前端回報的內容區塊（CSS px，相對於視窗）：x, y, w, h
    rect: Option<[f64; 4]>,
    hovering: bool,
    poller_running: bool,
    /// 這次的通知框有按鈕（發送到… / 下載）：游標移上去時要能點擊
    interactive: bool,
    /// 螢幕頂端有靈動島或瀏海（Mac）：通知框從那裡長出來
    island: Option<Island>,
}

/// 給前端的鏡頭區大小（CSS 像素）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Island {
    width: f64,
    height: f64,
    /// island：靈動島（四邊圓角的黑色膠囊）；notch：瀏海（貼著螢幕頂端）
    kind: &'static str,
}

/// 通知框右邊的按鈕。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Action {
    /// 複製了檔案或圖片：「發送到…」選一台裝置傳過去
    Send,
    /// 其他裝置經由 Google Drive 傳來檔案：「下載」
    Download { mid: String, from: String, size: u64 },
}

/// 有按鈕的通知框至少停留這麼久，來得及把游標移過去。
const ACTION_MIN_MS: u32 = 3000;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ShowEvent<'a> {
    seq: u64,
    payload: &'a crate::classify::Payload,
    source: Option<&'a str>,
    /// 從其他電腦即時同步過來：那台電腦的名稱
    via: Option<&'a str>,
    count: u32,
    anchor: Anchor,
    /// pill：螢幕正上方膠囊；bubble：游標旁微氣泡；card：螢幕四角
    variant: &'static str,
    minimal: bool,
    opacity: f32,
    duration_ms: u32,
    sound: bool,
    platform: &'static str,
    lang: crate::i18n::Lang,
    action: Option<&'a Action>,
    island: Option<Island>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct HoverEvent {
    inside: bool,
    history: Vec<HistoryOut>,
}

/// 複製的是檔案或圖片、而且有其他裝置可以傳：顯示「發送到…」。
fn send_action(app: &AppHandle, clip: &Clip) -> Option<Action> {
    let sendable = matches!(clip.payload, Payload::Files { .. } | Payload::Image { .. });
    (sendable && clip.via.is_none() && crate::cloud::transfer::has_peers(app)).then_some(Action::Send)
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let win = WebviewWindowBuilder::new(app, HUD_LABEL, WebviewUrl::App("hud.html".into()))
        .title("CopyC")
        .inner_size(HUD_W, HUD_H)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .focused(false)
        .focusable(false)
        // 「發送到…」等按鈕：視窗不在前景時第一下點擊就要有反應（macOS）
        .accept_first_mouse(true)
        .visible(false)
        .build()?;
    // 滑鼠穿透在各平台的 prepare_hud 裡設定（Linux 必須等視窗實際顯示後才能設）
    platform::prepare_hud(&win);
    Ok(())
}

/// 判斷這次複製是否為「連按」：同一內容，且通知框仍在畫面上或距上次不到 1 秒。
fn is_repeat(last_hash: Option<u64>, last_at: Option<Instant>, visible: bool, hash: u64, now: Instant) -> bool {
    last_hash == Some(hash) && (visible || last_at.is_some_and(|t| now.duration_since(t) <= REPEAT_WINDOW))
}

/// 剪貼簿管線的終點：記錄歷史、套用暫停 / 全螢幕規則後顯示。
pub fn handle(app: &AppHandle, outcome: Outcome) {
    let Outcome::Show(mut clip) = outcome else { return };
    let state = app.state::<AppState>();
    let settings = state.settings.lock().clone();
    let cloud = app.state::<crate::cloud::Cloud>();
    clip.via = cloud.take_incoming(clip.hash);

    let (count, repeat) = {
        let mut h = state.hud.lock();
        let now = Instant::now();
        let repeat = is_repeat(h.last_hash, h.last_at, h.visible, clip.hash, now);
        h.count = if repeat { h.count.saturating_add(1) } else { 1 };
        h.last_hash = Some(clip.hash);
        h.last_at = Some(now);
        (h.count, repeat)
    };
    state.record(&clip.payload, clip.via.as_deref().or(clip.source.as_deref()), repeat);
    let _ = app.emit("overview:changed", ());
    if let (Some(entry), Some(store)) = (&clip.entry, state.store.get()) {
        let at = crate::state::now_ms();
        // 其他電腦同步來的內容標上那台電腦，不再備份（那台會自己備份）
        let saved = match &clip.via {
            Some(device) => store.add_remote(entry, clip.hash, at, device).map(drop),
            None => store.add(entry, clip.source.as_deref(), clip.hash, at, settings.history_limit),
        };
        match saved {
            Ok(()) => {
                let _ = app.emit("history:changed", ());
                if clip.via.is_none() && entry.category.syncable() {
                    cloud.nudge();
                }
            }
            Err(e) => eprintln!("copyc: 無法寫入歷史紀錄：{e}"),
        }
    }
    // 同一份內容連按不重複傳送
    if let (None, Some(text), false) = (&clip.via, &clip.share, repeat) {
        cloud.send_live(text);
    }

    if settings.paused {
        return;
    }
    let minimal = match settings.fullscreen_action {
        FullscreenAction::Normal => false,
        action => {
            if !platform::is_fullscreen() {
                false
            } else if action == FullscreenAction::Pause {
                return;
            } else {
                true
            }
        }
    };
    let action = send_action(app, &clip);
    present(app, &clip, count, &settings, minimal, action);
}

/// 其他裝置經由 Google Drive 傳來檔案：通知框顯示檔名與「下載」按鈕。
pub fn incoming_file(app: &AppHandle, mid: &str, name: &str, size: u64, from: &str) {
    let settings = app.state::<AppState>().settings.lock().clone();
    if settings.paused {
        return;
    }
    let ext = std::path::Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let clip = Clip {
        payload: Payload::Files { first: name.to_string(), count: 1, is_dir: false, ext, icon: None },
        hash: rand::random(),
        source: None,
        entry: None,
        share: None,
        via: Some(from.to_string()),
    };
    let action = Action::Download { mid: mid.to_string(), from: from.to_string(), size };
    let app = app.clone();
    // 從背景工作呼叫：定位要在主執行緒以外做（present 會同步呼叫主執行緒）
    std::thread::spawn(move || {
        let count = {
            let state = app.state::<AppState>();
            let mut h = state.hud.lock();
            h.count = 1;
            h.last_hash = Some(clip.hash);
            h.last_at = Some(Instant::now());
            1
        };
        present(&app, &clip, count, &settings, false, Some(action));
    });
}

/// 「試一下」預覽：不寫入歷史，也不受暫停 / 全螢幕影響。
pub fn preview(app: &AppHandle, clip: &Clip, minimal: bool) {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().clone();
    let count = {
        let mut h = state.hud.lock();
        let now = Instant::now();
        let repeat = is_repeat(h.last_hash, h.last_at, h.visible, clip.hash, now);
        h.count = if repeat { h.count.saturating_add(1) } else { 1 };
        h.last_hash = Some(clip.hash);
        h.last_at = Some(now);
        h.count
    };
    present(app, clip, count, &settings, minimal, None);
}

fn present(app: &AppHandle, clip: &Clip, count: u32, settings: &Settings, minimal: bool, action: Option<Action>) {
    let action = action.filter(|_| !minimal);
    let Some(win) = app.get_webview_window(HUD_LABEL) else { return };
    let state = app.state::<AppState>();

    let (seq, was_visible, start_poller) = {
        let mut h = state.hud.lock();
        h.seq += 1;
        let was_visible = h.visible;
        h.visible = true;
        h.interactive = action.is_some();
        let start_poller = (settings.hover_history || action.is_some()) && !minimal && !h.poller_running;
        if start_poller {
            h.poller_running = true;
        }
        (h.seq, was_visible, start_poller)
    };

    // 定位會同步呼叫主執行緒（游標、螢幕、視窗大小），不能拿著 hud 鎖做：
    // 主執行緒上的 hud_rect / hud_finished 指令也要這把鎖，會互相等死。
    let (anchor, island) = if was_visible {
        let h = state.hud.lock();
        (h.anchor, h.island)
    } else {
        let (anchor, island) = place(app, &win, settings.position);
        let mut h = state.hud.lock();
        h.anchor = anchor;
        h.island = island;
        h.rect = None;
        h.hovering = false;
        (anchor, island)
    };

    if !was_visible {
        let w = win.clone();
        let _ = app.run_on_main_thread(move || platform::show_hud(&w));
    }

    let variant = match settings.position {
        Position::Top if island.is_some() => "island",
        Position::Top => "pill",
        Position::Cursor => "bubble",
        _ => "card",
    };
    let _ = app.emit_to(
        HUD_LABEL,
        "hud:show",
        ShowEvent {
            seq,
            payload: &clip.payload,
            source: clip.source.as_deref(),
            via: clip.via.as_deref(),
            count,
            anchor,
            variant,
            minimal,
            opacity: settings.opacity,
            duration_ms: if action.is_some() { settings.duration_ms.max(ACTION_MIN_MS) } else { settings.duration_ms },
            sound: settings.sound,
            platform: platform::PLATFORM,
            lang: crate::i18n::resolve(settings.language),
            action: action.as_ref(),
            island,
        },
    );

    if start_poller {
        spawn_hover_poller(app.clone());
    }
}

/// 依設定把視窗擺到目標位置，回傳內容在視窗中的對齊方式。
fn place(app: &AppHandle, win: &WebviewWindow, position: Position) -> (Anchor, Option<Island>) {
    let cursor = app.cursor_position().ok();
    let monitor = cursor
        .and_then(|c| app.monitor_from_point(c.x, c.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(m) = monitor else { return (Anchor::default(), None) };

    let s = m.scale_factor();
    let (w, h) = (HUD_W * s, HUD_H * s);
    let (mx, my) = (m.position().x as f64, m.position().y as f64);
    let (mw, mh) = (m.size().width as f64, m.size().height as f64);
    let wa = m.work_area();
    let (ax, ay) = (wa.position.x as f64, wa.position.y as f64);
    let (aw, ah) = (wa.size.width as f64, wa.size.height as f64);

    // Mac 的靈動島 / 瀏海：通知框貼著螢幕最上緣、對準鏡頭區的中央
    let housing = if position == Position::Top { platform::camera_housing(app, mx, mw, mh) } else { None };
    let island = housing.map(|hs| Island {
        width: hs.width / s,
        height: hs.height / s,
        kind: if hs.island { "island" } else { "notch" },
    });
    let (x, y, anchor) = match (position, housing) {
        (Position::Top, Some(hs)) => (mx + hs.left + hs.width / 2.0 - w / 2.0, my, Anchor { h: HAlign::Center, v: VAlign::Top }),
        _ => match position {
        Position::Top => (mx + (mw - w) / 2.0, ay, Anchor { h: HAlign::Center, v: VAlign::Top }),
        Position::TopLeft => (ax, ay, Anchor { h: HAlign::Left, v: VAlign::Top }),
        Position::TopRight => (ax + aw - w, ay, Anchor { h: HAlign::Right, v: VAlign::Top }),
        Position::BottomLeft => (ax, ay + ah - h, Anchor { h: HAlign::Left, v: VAlign::Bottom }),
        Position::BottomRight => (ax + aw - w, ay + ah - h, Anchor { h: HAlign::Right, v: VAlign::Bottom }),
        Position::Cursor => {
            let (cx, cy) = cursor.map(|c| (c.x, c.y)).unwrap_or((mx + mw / 2.0, my + mh / 2.0));
            let pad = STAGE_PAD * s;
            // 預設在游標右上方；超出螢幕右緣改到左側，太靠近頂端改到下方
            let mut anchor = Anchor { h: HAlign::Left, v: VAlign::Bottom };
            let mut x = cx + 14.0 * s - pad;
            if x + w > mx + mw {
                x = cx - 10.0 * s - (w - pad);
                anchor.h = HAlign::Right;
            }
            let mut y = cy - 8.0 * s - (h - pad);
            if cy - 110.0 * s < my {
                y = cy + 24.0 * s - pad;
                anchor.v = VAlign::Top;
            }
            (x, y, anchor)
        }
        },
    };

    let _ = win.set_size(PhysicalSize::new(w.round() as u32, h.round() as u32));
    let _ = win.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
    (anchor, island)
}

/// 通知框是滑鼠穿透的，收不到 hover 事件；改由這裡輪詢游標位置，
/// 讓「懸停展開最近 3 筆」與穿透點擊同時成立。只在通知框顯示期間運作。
fn spawn_hover_poller(app: AppHandle) {
    thread::spawn(move || {
        let state = app.state::<AppState>();
        loop {
            thread::sleep(HOVER_POLL);
            let (rect, hovering, interactive) = {
                let mut h = state.hud.lock();
                if !h.visible {
                    h.poller_running = false;
                    return;
                }
                (h.rect, h.hovering, h.interactive)
            };
            let (Some(r), Some(win)) = (rect, app.get_webview_window(HUD_LABEL)) else { continue };
            let (Ok(c), Ok(p), Ok(s)) = (app.cursor_position(), win.outer_position(), win.scale_factor()) else {
                continue;
            };
            let (left, top) = (p.x as f64 + r[0] * s, p.y as f64 + r[1] * s);
            let inside = c.x >= left && c.x <= left + r[2] * s && c.y >= top && c.y <= top + r[3] * s;
            if inside != hovering {
                state.hud.lock().hovering = inside;
                // 有按鈕時，游標在通知框上才接收點擊；移開就恢復滑鼠穿透
                if interactive {
                    let _ = win.set_ignore_cursor_events(!inside);
                }
                let history_on = state.settings.lock().hover_history;
                let history = if inside && history_on { state.history_out(1, 3) } else { Vec::new() };
                let _ = app.emit_to(HUD_LABEL, "hud:hover", HoverEvent { inside, history });
            }
        }
    });
}

pub fn finished(app: &AppHandle, seq: u64) {
    let state = app.state::<AppState>();
    {
        let mut h = state.hud.lock();
        if h.seq != seq || !h.visible {
            return;
        }
        h.visible = false;
        h.hovering = false;
        h.interactive = false;
    }
    if let Some(win) = app.get_webview_window(HUD_LABEL) {
        let _ = win.set_ignore_cursor_events(true);
        let w = win.clone();
        let _ = app.run_on_main_thread(move || platform::hide_hud(&w));
    }
}

pub fn set_rect(app: &AppHandle, rect: [f64; 4]) {
    app.state::<AppState>().hud.lock().rect = Some(rect);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_requires_same_content() {
        let now = Instant::now();
        assert!(!is_repeat(Some(1), Some(now), true, 2, now));
        assert!(!is_repeat(None, None, false, 1, now));
    }

    #[test]
    fn repeat_within_one_second_or_while_visible() {
        let t0 = Instant::now();
        assert!(is_repeat(Some(7), Some(t0), false, 7, t0 + Duration::from_millis(900)));
        assert!(!is_repeat(Some(7), Some(t0), false, 7, t0 + Duration::from_millis(1500)));
        assert!(is_repeat(Some(7), Some(t0), true, 7, t0 + Duration::from_millis(1500)));
    }
}
