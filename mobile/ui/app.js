// CopyC Android 的畫面：登入、把剪貼簿傳到電腦、最近／歷史紀錄（可搜尋與分類篩選）、設定。
// 文字在 i18n.js（T、LANG、LOCALE）。
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (s) => document.querySelector(s);
const bridge = window.CopyCAndroid;

document.documentElement.lang = LANG;
document.querySelectorAll('[data-i18n]').forEach((el) => {
  const v = T[el.dataset.i18n];
  if (typeof v === 'string') el.textContent = v;
});
document.querySelectorAll('[data-i18n-placeholder]').forEach((el) => (el.placeholder = T[el.dataset.i18nPlaceholder]));
document.querySelectorAll('[data-i18n-label]').forEach((el) => el.setAttribute('aria-label', T[el.dataset.i18nLabel]));

/* ---------------- 小工具 ---------------- */

let toastTimer = 0;
function toast(text) {
  const el = $('#toast');
  el.textContent = text;
  el.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove('show'), 2200);
}

const errText = (e) => T.err[typeof e === 'string' ? e : ''] ?? T.err.network;

const timeText = (at) => {
  const d = new Date(at);
  const today = new Date().toDateString() === d.toDateString();
  return today
    ? d.toLocaleTimeString(LOCALE, { hour: '2-digit', minute: '2-digit' })
    : d.toLocaleDateString(LOCALE, { month: 'short', day: 'numeric' });
};

/** 和電腦版一樣的分類規則（驗證碼優先：4～8 位純數字）。 */
function category(text) {
  const s = text.trim();
  if (/^\d{4,8}$/.test(s)) return 'code';
  if (/^(https?:\/\/|www\.)\S+$/i.test(s)) return 'link';
  if (/^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.test(s) || /^(rgba?|hsla?)\(.+\)$/i.test(s)) return 'color';
  if (s.length <= 254 && /^(mailto:)?[\w.+-]+@[\w-]+(\.[\w-]+)+$/.test(s)) return 'email';
  const digits = (s.match(/\d/g) ?? []).length;
  if (/^(tel:)?\+?[\d\s().-]{7,24}$/.test(s) && !/^\d{4}[-/.]\d{1,2}[-/.]\d{1,2}$/.test(s) && digits >= 7 && digits <= 15) {
    return 'phone';
  }
  return 'text';
}

const CHECK = 'M5 12.5 10 17 19 7';
function svgIcon(d) {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(ns, 'path');
  path.setAttribute('d', d);
  svg.append(path);
  return svg;
}

/** 一則內容的列表項目。 */
function row(clip, meta) {
  const li = document.createElement('li');
  const text = document.createElement('div');
  text.className = 'text';
  text.textContent = clip.text;
  const info = document.createElement('div');
  info.className = `meta${clip.incoming ? ' in' : ''}`;
  info.textContent = meta;
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'copy press';
  btn.textContent = T.copy;
  let undo = 0;
  btn.addEventListener('click', async () => {
    if (!(await invoke('write_clipboard', { text: clip.text }).catch(() => false))) return;
    // 回應落在手指底下：按鈕自己變成「已複製」
    btn.classList.add('done');
    btn.replaceChildren(svgIcon(CHECK), T.copiedShort);
    clearTimeout(undo);
    undo = setTimeout(() => {
      btn.classList.remove('done');
      btn.textContent = T.copy;
    }, 1400);
  });
  li.append(text, btn, info);
  return li;
}

/* ---------------- 分頁 ---------------- */

const TABS = ['recent', 'history', 'settings'];
function selectTab(name) {
  $('.tabs').style.setProperty('--tab', TABS.indexOf(name));
  for (const t of TABS) {
    $(`#tab-${t}`).setAttribute('aria-selected', String(t === name));
    $(`#panel-${t}`).hidden = t !== name;
  }
  if (name === 'history') loadHistory(true);
}
TABS.forEach((t) => $(`#tab-${t}`).addEventListener('click', () => selectTab(t)));

/* ---------------- 最近：這支手機收到與送出的（只存在手機上） ---------------- */

const RECENT_KEY = 'copyc-recent';
const RECENT_MAX = 50;
let recent = [];
try {
  recent = JSON.parse(localStorage.getItem(RECENT_KEY)) ?? [];
} catch {
  recent = [];
}

function addRecent(clip) {
  recent = [clip, ...recent.filter((r) => !(r.text === clip.text && r.incoming === clip.incoming))].slice(0, RECENT_MAX);
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify(recent));
  } catch {}
  renderRecent();
}

function renderRecent() {
  $('#list').replaceChildren(
    ...recent.map((c) => row(c, `${c.incoming ? T.from(c.device) : T.sent} · ${timeText(c.at)}`)),
  );
  $('#empty').hidden = recent.length > 0;
}

/* ---------------- 歷史紀錄：各電腦備份到 Google Drive 的 ---------------- */

const CATS = ['all', 'text', 'link', 'email', 'phone', 'code', 'color'];
const PAGE = 50;
let history = [];
let histCat = 'all';
let histShown = PAGE;
let histLoading = false;
let histLoadedAt = 0;

function renderChips() {
  const counts = { all: history.length };
  for (const h of history) counts[h.cat] = (counts[h.cat] ?? 0) + 1;
  $('#chips').replaceChildren(
    ...CATS.filter((c) => c === 'all' || counts[c] || c === histCat).map((c) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.className = `chip press${c === histCat ? ' on' : ''}`;
      b.setAttribute('aria-pressed', String(c === histCat));
      const tick = document.createElement('span');
      tick.className = 'tick';
      tick.append(svgIcon(CHECK));
      const n = document.createElement('span');
      n.className = 'n';
      n.textContent = counts[c] ?? 0;
      b.append(tick, T[`cat.${c}`], n);
      b.addEventListener('click', () => {
        if (histCat === c) return;
        histCat = c;
        histShown = PAGE;
        // 先讓標籤自己長出對勾，再更新列表
        document.querySelectorAll('#chips .chip').forEach((x) => {
          x.classList.toggle('on', x === b);
          x.setAttribute('aria-pressed', String(x === b));
        });
        setTimeout(renderHistory, 240);
      });
      return b;
    }),
  );
}

function renderHistory() {
  renderChips();
  const q = $('#search').value.trim().toLowerCase();
  const matches = history.filter((h) => (histCat === 'all' || h.cat === histCat) && (!q || h.text.toLowerCase().includes(q)));
  $('#hist-list').replaceChildren(...matches.slice(0, histShown).map((h) => row(h, `${T.from(h.device)} · ${timeText(h.at)}`)));
  $('#hist-more').hidden = matches.length <= histShown;
  const state = $('#hist-state');
  if (histLoading && !history.length) state.textContent = T['hist.loading'];
  else if (!history.length) state.textContent = T['hist.empty'];
  else if (!matches.length) state.textContent = T['hist.none'];
  else state.textContent = histLoading ? T['hist.loading'] : T['hist.count'](matches.length);
}

const withCat = (items) => items.map((h) => ({ ...h, cat: category(h.text) }));

async function loadHistory(refresh) {
  if (!history.length) history = withCat(await invoke('history_cached').catch(() => []));
  renderHistory();
  // 每次打開分頁最多一分鐘更新一次
  if (!refresh || histLoading || Date.now() - histLoadedAt < 60_000) return;
  histLoading = true;
  renderHistory();
  try {
    history = withCat(await invoke('history_refresh'));
    histLoadedAt = Date.now();
  } catch (e) {
    toast(errText(e));
  }
  histLoading = false;
  renderHistory();
}

$('#search').addEventListener('input', () => {
  histShown = PAGE;
  renderHistory();
});
$('#hist-more').addEventListener('click', () => {
  histShown += PAGE;
  renderHistory();
});

/* ---------------- 狀態與設定 ---------------- */

let status = null;
let lastError = null;

async function refresh() {
  status = await invoke('status').catch(() => null);
  if (!status) return;
  const signingIn = status.signingIn;
  $('#unavailable').hidden = status.available;
  $('#signed-out').hidden = !status.available || status.signedIn;
  $('#sign-in').disabled = signingIn;
  $('#sign-in-spin').hidden = !signingIn;
  $('#sign-in-text').textContent = signingIn ? T['out.signingIn'] : T['out.signIn'];
  $('#signed-in').hidden = !status.signedIn;
  const pill = $('#pill');
  pill.hidden = !status.signedIn;
  pill.classList.toggle('on', status.live === 'on');
  const live = T.live[status.live] ?? T.live.off;
  $('#pill-text').textContent = live;
  if (status.signedIn) $('#account').textContent = T.signedInAs(status.email ?? '');
  $('#send').disabled = status.live !== 'on' && !status.lanPeers;
  $('#pref-bg').checked = status.prefs.background;
  $('#pref-lan').checked = status.prefs.lan;
  const needPerm = status.prefs.lan && bridge && !bridge.hasLocalNetwork();
  const note = $('#lan-note');
  note.hidden = !status.prefs.lan || (!needPerm && !status.lanPeers);
  note.textContent = needPerm ? T['set.lanPerm'] : T['set.lanPeers'](status.lanPeers);
  if (status.error && status.error !== lastError) toast(errText(status.error));
  lastError = status.error;
  if (status.live === 'on' || status.lanPeers) runPending();
}

function savePrefs() {
  invoke('set_prefs', { prefs: { background: $('#pref-bg').checked, lan: $('#pref-lan').checked } });
}

$('#pref-bg').addEventListener('change', (e) => {
  if (e.target.checked) bridge?.requestNotifications();
  savePrefs();
});
$('#pref-lan').addEventListener('change', (e) => {
  if (e.target.checked && bridge && !bridge.hasLocalNetwork()) bridge.requestLocalNetwork();
  savePrefs();
});

/* ---------------- 語言（Select：8 個固定選項選一個，選了馬上套用） ---------------- */

const langSheet = $('#lang-sheet');
function closeLangSheet() {
  document.body.classList.remove('sheet-open');
  setTimeout(() => (langSheet.hidden = true), 380);
}
function openLangSheet() {
  langSheet.hidden = false;
  requestAnimationFrame(() => requestAnimationFrame(() => document.body.classList.add('sheet-open')));
  $('#lang-options [aria-selected="true"]')?.focus();
}

// 只有 Android 上才能換（電腦上開發時沒有 bridge）
if (bridge?.setLanguage) {
  const current = APP_LANG.split(',')[0];
  const options = [['', T['set.langSystem']], ...Object.entries(LANG_NAMES)];
  const currentName = options.find(([tag]) => tag === current)?.[1] ?? T['set.langSystem'];
  document.querySelectorAll('.lang-name').forEach((el) => (el.textContent = currentName));
  $('#lang-options').replaceChildren(
    ...options.map(([tag, name]) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'option press';
      b.setAttribute('role', 'option');
      b.setAttribute('aria-selected', String(tag === current));
      if (tag) b.lang = tag;
      const label = document.createElement('span');
      label.textContent = name;
      b.append(label, svgIcon(CHECK));
      b.addEventListener('click', () => {
        if (tag === current) return closeLangSheet();
        $('#lang-options [aria-selected="true"]')?.setAttribute('aria-selected', 'false');
        b.setAttribute('aria-selected', 'true');
        bridge.setLanguage(tag);
        closeLangSheet();
        // 選單收好、系統記住新語言之後，用新語言重新載入畫面
        setTimeout(() => location.reload(), 420);
      });
      return b;
    }),
  );
  document.querySelectorAll('.lang-open').forEach((el) => el.addEventListener('click', openLangSheet));
  $('.sheet-scrim').addEventListener('click', closeLangSheet);
  document.querySelectorAll('.lang-setting, .lang-foot').forEach((el) => (el.hidden = false));
}

/* ---------------- 登入／登出 ---------------- */

$('#sign-in').addEventListener('click', async () => {
  try {
    await invoke('sign_in');
  } catch (e) {
    if (e !== 'cancelled') toast(errText(e));
  }
  refresh();
});

let signOutArmed = 0;
$('#sign-out').addEventListener('click', async (e) => {
  const btn = e.currentTarget;
  if (!signOutArmed) {
    btn.textContent = T['in.signOutConfirm'];
    signOutArmed = setTimeout(() => {
      signOutArmed = 0;
      btn.textContent = T['in.signOut'];
    }, 3000);
    return;
  }
  clearTimeout(signOutArmed);
  signOutArmed = 0;
  btn.textContent = T['in.signOut'];
  await invoke('sign_out');
  history = [];
  // 「最近」也是這個帳號的內容，登出時一起刪除
  recent = [];
  try {
    localStorage.removeItem(RECENT_KEY);
  } catch {}
  renderRecent();
  selectTab('recent');
  refresh();
});

/* ---------------- 傳到電腦 ---------------- */

async function send(text, auto = false) {
  try {
    const sent = await invoke('send_text', { text });
    if (sent.echo) return auto || toast(T.echo);
    if (!sent.clip) return auto || toast(T.emptyClip);
    addRecent(sent.clip);
    if (auto) return toast(T.autoSent);
    const btn = $('#send');
    btn.classList.add('sent');
    setTimeout(() => btn.classList.remove('sent'), 420);
    toast(T.sentToast);
  } catch (e) {
    toast(errText(e));
  }
}

/** 讀一次剪貼簿：優先用 Android 原生的讀法（帶格式的文字也讀得到），電腦上開發時改用外掛。 */
async function readOnce() {
  if (bridge?.readClipboard) {
    try {
      return bridge.readClipboard() || null;
    } catch {
      return null;
    }
  }
  return invoke('read_clipboard').catch(() => null);
}

/** Android 只讓前景、而且有焦點的 App 讀剪貼簿：剛打開時多試幾次。 */
async function readClipboard() {
  for (let i = 0; i < 4; i++) {
    const text = await readOnce();
    if (text) return text;
    await new Promise((r) => setTimeout(r, 250));
  }
  return null;
}

$('#send').addEventListener('click', async () => {
  const text = await readClipboard();
  if (!text) return toast(T.emptyClip);
  send(text);
});

// 從其他 App「分享 → CopyC」傳來的文字，或快速設定的「傳到電腦」：連上之後再做
let sharedText = null;
let sendClipboardPending = false;
function takePending() {
  const text = bridge?.takeShared?.();
  if (text) sharedText = text;
  if (bridge?.takeAction?.() === 'sendClipboard') sendClipboardPending = true;
}
async function runPending() {
  if (sharedText) {
    const text = sharedText;
    sharedText = null;
    send(text);
  }
  if (sendClipboardPending && document.hasFocus()) {
    sendClipboardPending = false;
    const text = await readClipboard();
    if (text) send(text);
    else toast(T.emptyClip);
  }
}
window.addEventListener('focus', () => {
  if (status && (status.live === 'on' || status.lanPeers)) runPending();
  autoSend();
});

/* ---------------- 回到 CopyC 時自動傳送剪貼簿 ---------------- */

// Android 不讓 App 在背景讀剪貼簿，所以在使用者回到 CopyC 的那一刻讀一次：
// 內容是新的（不是剛傳過、也不是剛從電腦收到的），就自動傳到電腦。
const AUTO_KEY = 'copyc-autosend';
const AUTO_LAST = 'copyc-autosend-last';
let autoOn = true;
try {
  autoOn = localStorage.getItem(AUTO_KEY) !== '0';
} catch {}
$('#pref-auto').checked = autoOn;
$('#pref-auto').addEventListener('change', (e) => {
  autoOn = e.target.checked;
  try {
    localStorage.setItem(AUTO_KEY, autoOn ? '1' : '0');
  } catch {}
  if (autoOn) autoSend();
});

/** 只存雜湊，不把剪貼簿內容留在手機上。 */
function hashText(text) {
  let h = 2166136261;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 16777619);
  return (h >>> 0).toString(36) + ':' + text.length;
}

let autoBusy = false;
async function autoSend() {
  if (!autoOn || autoBusy || !status?.signedIn || document.visibilityState !== 'visible') return;
  if (sharedText || sendClipboardPending) return; // 分享或快速設定按鈕的內容會另外處理
  autoBusy = true;
  try {
    const text = await readClipboard();
    if (!text || bridge?.clipboardSensitive?.()) return;
    const h = hashText(text);
    let last = null;
    try {
      last = localStorage.getItem(AUTO_LAST);
    } catch {}
    if (h === last || recent.some((r) => r.text === text)) return;
    try {
      localStorage.setItem(AUTO_LAST, h);
    } catch {}
    await send(text, true);
  } finally {
    autoBusy = false;
  }
}

/* ---------------- 事件 ---------------- */

listen('status', refresh);
listen('clip', (e) => {
  addRecent(e.payload);
  toast(T.received(e.payload.device));
});

document.addEventListener('visibilitychange', () => {
  if (document.visibilityState !== 'visible') {
    invoke('pause').catch(() => {});
    return;
  }
  takePending();
  invoke('resume').catch(() => {});
  refresh().then(autoSend);
});

renderRecent();
takePending();
invoke('resume').catch(() => {});
refresh().then(autoSend);
