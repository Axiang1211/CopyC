// 通知框前端：接收 hud:show / hud:hover，負責動畫、計時與回報內容區塊位置。
import { buildLead, compose, fill, summary, timeAgo } from './shared/compose.js';
import { applyI18n, setLang } from './shared/i18n.js';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const hud = $('hud');
const main = $('main');
const copy = $('copy');
const leadSlot = $('lead');
const titleEl = $('title');
const badgeEl = $('badge');
const detailEl = $('detail');
const trayList = $('tray-list');
const dot = $('dot');

const EXIT_MS = 240;
const HOVER_LEAVE_MS = 800;

let seq = 0;
let shown = false;
let hovering = false;
let lastKey = '';
let hideTimer = 0;
let exitTimer = 0;
let audio = null;
let lastEvent = null;
let hoverItems = [];

function restart(el, cls) {
  el.classList.remove(cls);
  void el.offsetWidth; // 強制 reflow 讓動畫可重播
  el.classList.add(cls);
}

function render(ev) {
  const view = compose(ev.payload, ev.count, ev.via);
  leadSlot.replaceChildren(buildLead(view.lead));
  fill(titleEl, view.title);
  fill(detailEl, view.detail);
  if (view.badge) {
    badgeEl.textContent = view.badge;
    badgeEl.hidden = false;
    restart(badgeEl, 'bump');
  } else {
    badgeEl.hidden = true;
  }
}

function renderHistory(items) {
  trayList.replaceChildren(
    ...items.map((item, i) => {
      const li = document.createElement('li');
      li.style.setProperty('--i', i);
      const view = compose(item.payload);
      const text = document.createElement('span');
      text.className = 'text';
      fill(text, summary(item.payload));
      const time = document.createElement('time');
      time.textContent = timeAgo(item.at);
      li.append(buildLead(view.lead, 'mini'), text, time);
      return li;
    }),
  );
}

function reportRect() {
  const target = document.body.classList.contains('is-minimal') ? dot : hud;
  const r = target.getBoundingClientRect();
  invoke('hud_rect', { x: r.left, y: r.top, w: r.width, h: r.height }).catch(() => {});
}

function schedule(ms) {
  clearTimeout(hideTimer);
  hideTimer = setTimeout(leave, ms);
}

function leave() {
  if (hovering) return;
  hud.classList.remove('is-entering', 'is-shake');
  hud.classList.add('is-out');
  dot.classList.add('is-out');
  clearTimeout(exitTimer);
  exitTimer = setTimeout(() => {
    reset();
    const done = seq;
    // 等透明的最後一幀畫完再請 Rust 隱藏視窗，下次顯示時才不會閃出舊內容
    requestAnimationFrame(() => requestAnimationFrame(() => invoke('hud_finished', { seq: done }).catch(() => {})));
  }, EXIT_MS);
}

function reset() {
  shown = false;
  hovering = false;
  lastKey = '';
  hud.className = 'hud';
  dot.className = 'dot';
  trayList.replaceChildren();
}

function tick(repeat) {
  try {
    audio ??= new AudioContext();
    const t = audio.currentTime;
    const osc = audio.createOscillator();
    const gain = audio.createGain();
    osc.type = 'sine';
    osc.frequency.setValueAtTime(repeat ? 1318 : 1046, t);
    osc.frequency.exponentialRampToValueAtTime(repeat ? 1760 : 1568, t + 0.05);
    gain.gain.setValueAtTime(0.0001, t);
    gain.gain.exponentialRampToValueAtTime(0.07, t + 0.008);
    gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.13);
    osc.connect(gain).connect(audio.destination);
    osc.start(t);
    osc.stop(t + 0.14);
  } catch {
    /* 沒有音訊裝置時靜默略過 */
  }
}

function show(ev) {
  lastEvent = ev;
  seq = ev.seq;
  setLang(ev.lang);
  applyI18n();
  clearTimeout(exitTimer);
  document.body.dataset.h = ev.anchor.h;
  document.body.dataset.v = ev.anchor.v;
  document.body.dataset.platform = ev.platform;
  document.body.classList.toggle('is-minimal', ev.minimal);
  document.documentElement.style.setProperty('--alpha', String(ev.opacity));

  const key = JSON.stringify(ev.payload);
  const wasShown = shown && !hud.classList.contains('is-out');
  hud.dataset.variant = ev.variant;
  hud.classList.remove('is-out');
  dot.classList.remove('is-out');

  if (ev.minimal) {
    restart(dot, 'is-on');
  } else {
    render(ev);
    hud.classList.add('is-shown');
    if (!wasShown) {
      hud.classList.remove('is-shake', 'is-hover');
      restart(hud, 'is-entering');
    } else if (ev.count > 1) {
      // 連按：不關閉重開，原地震一下並更新次數
      hud.classList.remove('is-entering');
      restart(hud, 'is-shake');
    } else if (key !== lastKey) {
      restart(copy, 'is-swap');
    }
  }

  lastKey = key;
  shown = true;
  if (ev.sound) tick(ev.count > 1);
  if (!hovering) schedule(ev.durationMs);
  requestAnimationFrame(reportRect);
}

function onHover({ inside, history }) {
  hoverItems = history;
  if (!shown) return;
  hovering = inside;
  if (inside) {
    clearTimeout(hideTimer);
    if (history.length) {
      renderHistory(history);
      hud.classList.add('is-hover');
    }
  } else {
    hud.classList.remove('is-hover');
    schedule(HOVER_LEAVE_MS);
  }
}

hud.addEventListener('animationend', (e) => {
  if (e.target !== hud && e.target !== main) return;
  if (e.animationName === 'shake') hud.classList.remove('is-shake');
  if (['pill-drop', 'bubble-in', 'card-in', 'card-in-up'].includes(e.animationName)) {
    hud.classList.remove('is-entering');
  }
});

// 托盤展開 / 內容變寬時同步回報區塊，讓懸停判斷跟著實際範圍走
new ResizeObserver(() => shown && reportRect()).observe(hud);

/* ---------- 新版本檢查 ---------- */

// 通知框頁面從啟動起就常駐（隱藏），所以由這裡負責：啟動 3 秒後查一次，之後每天再查一次。
const UPDATE_DELAY_MS = 3000;
const UPDATE_EVERY_MS = 24 * 60 * 60 * 1000;
const LATEST_URL = 'https://api.github.com/repos/Axiang1211/CopyC/releases/latest';

const parseVersion = (v) => v.replace(/^v/, '').split('.').map((n) => Number.parseInt(n, 10) || 0);

function isNewer(latest, current) {
  const [a, b] = [parseVersion(latest), parseVersion(current)];
  for (let i = 0; i < 3; i++) {
    if ((a[i] ?? 0) !== (b[i] ?? 0)) return (a[i] ?? 0) > (b[i] ?? 0);
  }
  return false;
}

async function checkUpdate() {
  try {
    const { enabled, current } = await invoke('update_check');
    if (!enabled) return;
    const res = await fetch(LATEST_URL, { headers: { Accept: 'application/vnd.github+json' } });
    if (!res.ok) return;
    const { tag_name: tag } = await res.json();
    if (typeof tag === 'string' && /^v?\d+(\.\d+){0,2}$/.test(tag) && isNewer(tag, current)) {
      await invoke('update_found', { version: tag.replace(/^v/, '') });
    }
  } catch {
    /* 離線或被限流時略過，下次再查 */
  }
}

setTimeout(checkUpdate, UPDATE_DELAY_MS);
setInterval(checkUpdate, UPDATE_EVERY_MS);

listen('hud:show', (e) => show(e.payload));
listen('hud:hover', (e) => onHover(e.payload));
listen('settings:changed', async () => {
  const overview = await invoke('get_overview');
  setLang(overview.lang);
  applyI18n();
  if (shown && lastEvent) {
    render(lastEvent);
    if (hovering) renderHistory(hoverItems);
    requestAnimationFrame(reportRect);
  }
});
