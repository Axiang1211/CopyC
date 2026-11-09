// CopyC Android 原型的畫面：登入、把剪貼簿傳到電腦、最近收到與送出的內容。
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (s) => document.querySelector(s);

/* ---------------- 文字（繁中與英文；正式版再加其他語言） ---------------- */

const ZH = {
  tagline: '一台複製，另一台貼上',
  'unavailable.title': '這個版本還不能登入',
  'unavailable.body': '建置時沒有設定 Android 用的 Google 用戶端。',
  'out.title': '連接你的電腦',
  'out.body': '用電腦版 CopyC 登入的同一個 Google 帳號登入。電腦上複製的文字會直接出現在手機的剪貼簿，手機的內容也能傳到電腦。',
  'out.signIn': '用 Google 登入',
  'out.privacy': '內容在裝置上加密，伺服器看不到。',
  'code.title': '在 Google 輸入這組代碼',
  'code.copied': '代碼已複製，開啟頁面後貼上即可。',
  'code.open': '開啟 google.com/device',
  'code.wait': '完成後回到這裡，會自動繼續',
  'code.cancel': '取消',
  'in.send': '把剪貼簿傳到電腦',
  'in.hint': '電腦上複製的內容，會在 App 開著時直接放進手機的剪貼簿。在其他 App 也可以用「分享」選 CopyC 傳到電腦。',
  'in.recent': '最近',
  'in.empty': '還沒有內容。到電腦上複製一段文字試試看。',
  'in.signOut': '登出',
  'in.signOutConfirm': '再按一次登出',
  foot: 'CopyC Android 原型 · 免費',
  live: { on: '已連線', connecting: '連線中…', off: '未連線' },
  from: (d) => `來自 ${d}`,
  sent: '已傳到電腦',
  copy: '複製',
  copied: '已放進剪貼簿',
  sentToast: '已傳到電腦',
  received: (d) => `收到來自 ${d} 的內容，已放進剪貼簿`,
  emptyClip: '剪貼簿是空的，先複製一段文字',
  echo: '這是剛從電腦收到的內容',
  err: {
    offline: '還沒連上，請稍候再試',
    tooLong: '內容太長（上限 64 KB）',
    network: '網路有問題，請稍後再試',
    signedOut: '登入已失效，請重新登入',
    unavailable: '同步服務暫時無法使用',
    rateLimited: '太頻繁了，請稍後再試',
    cancelled: '登入已取消',
  },
  signedInAs: (e) => `已登入：${e}`,
};

const EN = {
  tagline: 'Copy here. Paste there.',
  'unavailable.title': 'Sign-in isn’t set up in this build',
  'unavailable.body': 'This build has no Google client for Android.',
  'out.title': 'Connect your computers',
  'out.body': 'Sign in with the same Google account you use in CopyC on your computer. Text you copy there lands on your phone’s clipboard, and you can send your phone’s clipboard back.',
  'out.signIn': 'Sign in with Google',
  'out.privacy': 'Content is encrypted on your devices; the server can’t read it.',
  'code.title': 'Enter this code at Google',
  'code.copied': 'The code is copied — just paste it on the page.',
  'code.open': 'Open google.com/device',
  'code.wait': 'Come back here when you’re done',
  'code.cancel': 'Cancel',
  'in.send': 'Send clipboard to computer',
  'in.hint': 'While the app is open, text you copy on your computer goes straight to your phone’s clipboard. In other apps, use Share → CopyC to send text to your computer.',
  'in.recent': 'Recent',
  'in.empty': 'Nothing yet. Copy some text on your computer to try it.',
  'in.signOut': 'Sign out',
  'in.signOutConfirm': 'Tap again to sign out',
  foot: 'CopyC for Android · prototype · free',
  live: { on: 'Connected', connecting: 'Connecting…', off: 'Offline' },
  from: (d) => `From ${d}`,
  sent: 'Sent to computer',
  copy: 'Copy',
  copied: 'On your clipboard',
  sentToast: 'Sent to your computer',
  received: (d) => `Received from ${d} — on your clipboard`,
  emptyClip: 'Your clipboard is empty — copy some text first',
  echo: 'That’s what you just received from your computer',
  err: {
    offline: 'Not connected yet — try again in a moment',
    tooLong: 'Too long (64 KB max)',
    network: 'Network problem — try again later',
    signedOut: 'Signed out — please sign in again',
    unavailable: 'Sync is unavailable right now',
    rateLimited: 'Too many requests — try again later',
    cancelled: 'Sign-in cancelled',
  },
  signedInAs: (e) => `Signed in as ${e}`,
};

const T = /^zh/i.test(navigator.language) ? ZH : EN;
document.documentElement.lang = T === ZH ? 'zh-Hant' : 'en';
document.querySelectorAll('[data-i18n]').forEach((el) => {
  const v = T[el.dataset.i18n];
  if (typeof v === 'string') el.textContent = v;
});

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

const timeText = (at) =>
  new Date(at).toLocaleTimeString(T === ZH ? 'zh-TW' : 'en-US', { hour: '2-digit', minute: '2-digit' });

/* ---------------- 最近的內容（只存在這支手機的 App 裡） ---------------- */

const RECENT_KEY = 'copyc-recent';
const RECENT_MAX = 30;
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
  const list = $('#list');
  const items = recent.map((clip) => {
    const li = document.createElement('li');
    const text = document.createElement('div');
    text.className = 'text';
    text.textContent = clip.text;
    const meta = document.createElement('div');
    meta.className = `meta${clip.incoming ? ' in' : ''}`;
    meta.textContent = `${clip.incoming ? T.from(clip.device) : T.sent} · ${timeText(clip.at)}`;
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.textContent = T.copy;
    btn.addEventListener('click', async () => {
      if (await invoke('write_clipboard', { text: clip.text }).catch(() => false)) toast(T.copied);
    });
    li.append(text, btn, meta);
    return li;
  });
  list.replaceChildren(...items);
  $('#empty').hidden = recent.length > 0;
}

/* ---------------- 狀態 ---------------- */

let status = null;

async function refresh() {
  status = await invoke('status').catch(() => null);
  if (!status) return;
  const signingIn = status.signingIn;
  $('#unavailable').hidden = status.available;
  $('#signed-out').hidden = !status.available || status.signedIn || signingIn;
  $('#code-card').hidden = !signingIn;
  $('#signed-in').hidden = !status.signedIn;
  const pill = $('#pill');
  pill.hidden = !status.signedIn;
  pill.classList.toggle('on', status.live === 'on');
  $('#pill-text').textContent = T.live[status.live] ?? T.live.off;
  if (status.signedIn) $('#account').textContent = T.signedInAs(status.email ?? '');
  $('#send').disabled = status.live !== 'on';
  if (status.error && status.error !== lastError) toast(errText(status.error));
  lastError = status.error;
  if (status.live === 'on') flushShared();
}
let lastError = null;

/* ---------------- 登入 ---------------- */

let verificationUrl = 'https://www.google.com/device';

$('#sign-in').addEventListener('click', async () => {
  try {
    const code = await invoke('sign_in_start');
    $('#code').textContent = code.userCode;
    verificationUrl = code.verificationUrl;
    await refresh();
  } catch (e) {
    toast(errText(e));
  }
});
$('#open-google').addEventListener('click', () => invoke('open_verification', { url: verificationUrl }));
$('#cancel').addEventListener('click', async () => {
  await invoke('sign_in_cancel');
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
  refresh();
});

/* ---------------- 傳到電腦 ---------------- */

async function send(text) {
  try {
    const sent = await invoke('send_text', { text });
    if (sent.echo) return toast(T.echo);
    if (!sent.clip) return toast(T.emptyClip);
    addRecent(sent.clip);
    toast(T.sentToast);
  } catch (e) {
    toast(errText(e));
  }
}

$('#send').addEventListener('click', async () => {
  const text = await invoke('read_clipboard').catch(() => null);
  if (!text) return toast(T.emptyClip);
  send(text);
});

// 從其他 App「分享 → CopyC」傳來的文字：MainActivity 先收著，連上之後送出
let shared = null;
function takeShared() {
  const text = window.CopyCAndroid?.takeShared?.();
  if (text) shared = text;
}
function flushShared() {
  if (!shared) return;
  const text = shared;
  shared = null;
  send(text);
}

/* ---------------- 事件 ---------------- */

listen('status', refresh);
listen('clip', (e) => {
  addRecent(e.payload);
  toast(T.received(e.payload.device));
});

document.addEventListener('visibilitychange', () => {
  if (document.visibilityState !== 'visible') return;
  takeShared();
  invoke('resume').catch(() => {});
  refresh();
});

renderRecent();
takeShared();
refresh();
