// CopyC 官網互動：真實複製試用、錨點導航、手風琴、下載卡片。
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const reduceMotion = matchMedia('(prefers-reduced-motion: reduce)').matches;

function restart(el, cls) {
  el.classList.remove(cls);
  void el.offsetWidth; // 強制 reflow，讓動畫可以重播
  el.classList.add(cls);
}

/* ---------------- 語言 ---------------- */

const EN = document.documentElement.lang.startsWith('en');
const T = EN
  ? {
      dlMac: 'Download for macOS',
      dlWin: 'Download for Windows',
      color: (v) => `Copied color: ${v}`,
      hex: 'HEX color',
      link: 'Link copied',
      text: (p) => `Copied: “${p}”`,
      stats: (c, l) => `${c} characters · ${l} line${l > 1 ? 's' : ''}`,
      again: 'Copied!',
      badge: (n) => `Confirmed ×${n}`,
      relax: 'Relax — it’s safely on your clipboard.',
      brewing: 'The first cup is still brewing — first release coming soon ☕',
      latest: (tag, date) => `Latest ${tag} · released ${date} · Free, no ads — click to download.`,
      dlArm: 'Download .exe installer (ARM64)',
      altX64Lead: 'Not on an ARM PC? ',
      altX64: 'Get the x64 version',
      locale: 'en-US',
      menuOpen: 'Open menu',
      menuClose: 'Close menu',
    }
  : {
      dlMac: '下載 macOS 版',
      dlWin: '下載 Windows 版',
      color: (v) => `你已成功複製色碼：${v}`,
      hex: 'HEX 色碼',
      link: '你已成功複製連結',
      text: (p) => `你已成功複製：「${p}」`,
      stats: (c, l) => `共 ${c} 字元 · ${l} 行`,
      again: '你已成功複製！',
      badge: (n) => `已確認 ×${n} 次`,
      relax: '放心吧！內容已經安全躺在剪貼簿裡了',
      brewing: '第一杯正在沖泡中，首個版本即將發布 ☕',
      latest: (tag, date) => `最新版本 ${tag} · ${date} 發布 · 免費、無廣告，點擊直接下載。`,
      dlArm: '下載 .exe 安裝程式（ARM64）',
      altX64Lead: '不是 ARM 電腦？',
      altX64: '下載 x64 版',
      locale: 'zh-TW',
      menuOpen: '開啟選單',
      menuClose: '關閉選單',
    };

/* ---------------- 平台偵測 ---------------- */

const ua = navigator.userAgentData?.platform || navigator.platform || navigator.userAgent;
const os = /mac|iphone|ipad/i.test(ua) ? 'macos' : /win/i.test(ua) ? 'windows' : 'other';
const isMac = os === 'macos';

$$('[data-mod]').forEach((k) => (k.textContent = isMac ? '⌘' : 'Ctrl'));
if (os !== 'other') {
  const card = $(`.platform-wrap[data-os="${os}"]`);
  card.classList.add('is-recommended');
  $('.recommend', card).hidden = false;
  $('#hero-download-label').textContent = isMac ? T.dlMac : T.dlWin;
}

// Windows on ARM：Chromium 系瀏覽器（Edge / Chrome）才回報 CPU 架構；偵測不到時維持 x64，
// Windows 11 on ARM 也能以模擬方式執行 x64 版
if (os === 'windows') {
  navigator.userAgentData
    ?.getHighEntropyValues(['architecture'])
    .then(({ architecture }) => {
      if (architecture !== 'arm') return;
      const main = $('#win-download');
      const alt = $('#arch-alt');
      const base = 'https://github.com/Axiang1211/CopyC/releases/latest/download/';
      main.href = base + 'CopyC-windows-arm64-setup.exe';
      main.dataset.asset = 'CopyC-windows-arm64-setup.exe';
      $('span', main).firstChild.textContent = T.dlArm;
      alt.href = base + 'CopyC-windows-x64-setup.exe';
      alt.textContent = T.altX64;
      $('#arch-note').firstChild.textContent = T.altX64Lead;
    })
    .catch(() => {});
}

/* ---------------- 頁面 HUD：跟 App 一樣的回饋 ---------------- */

const hud = $('#hud');
const hudLead = $('#hud-lead');
const hudTitle = $('#hud-title');
const hudDetail = $('#hud-detail');
const hudBadge = $('#hud-badge');
const CHECK = '<svg viewBox="0 0 24 24"><path d="M5.5 12.5l4.2 4.2L18.5 7.8"/></svg>';
const GLOBE =
  '<svg viewBox="0 0 24 24" style="stroke-width:1.8"><circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.4 2.5 3.6 5.3 3.6 8.5s-1.2 6-3.6 8.5c-2.4-2.5-3.6-5.3-3.6-8.5s1.2-6 3.6-8.5z"/></svg>';
const HEX = /^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;

let visible = false;
let lastText = '';
let lastAt = 0;
let count = 0;
let hideTimer = 0;
let outTimer = 0;

function render(text, n) {
  hudLead.className = 'hud-lead';
  hudLead.style.removeProperty('--c');
  hudDetail.replaceChildren();
  const flat = text.replace(/\s+/g, ' ').trim();

  if (HEX.test(flat)) {
    hudLead.classList.add('is-color');
    hudLead.style.setProperty('--c', flat);
    hudLead.innerHTML = '';
    hudTitle.textContent = T.color(flat.toUpperCase());
    hudDetail.textContent = T.hex;
  } else if (/^https?:\/\/\S+$/i.test(flat)) {
    const url = new URL(flat);
    hudLead.classList.add('is-url');
    hudLead.innerHTML = GLOBE;
    hudTitle.textContent = T.link;
    const domain = document.createElement('b');
    domain.textContent = url.host;
    const rest = url.pathname + url.search;
    hudDetail.append(`${url.protocol}//`, domain, rest.length > 22 ? `${rest.slice(0, 22)}…` : rest);
  } else {
    const chars = [...flat];
    const preview = chars.slice(0, 25).join('') + (chars.length > 25 ? '…' : '');
    hudLead.innerHTML = CHECK;
    hudTitle.textContent = T.text(preview);
    hudDetail.textContent = T.stats([...text].length, text.split(/\r?\n/).length);
  }

  if (n > 1) {
    hudTitle.textContent = T.again;
    hudBadge.textContent = T.badge(n);
    hudBadge.hidden = false;
    restart(hudBadge, 'bump');
    hudDetail.textContent = T.relax;
  } else {
    hudBadge.hidden = true;
  }
}

function showHud(text) {
  if (!text) return;
  const now = Date.now();
  const repeat = text === lastText && (visible || now - lastAt <= 1000);
  const changed = text !== lastText;
  count = repeat ? count + 1 : 1;
  lastText = text;
  lastAt = now;
  clearTimeout(hideTimer);
  clearTimeout(outTimer);
  render(text, count);

  if (!visible) {
    hud.className = 'hud';
    restart(hud, 'is-in');
  } else if (repeat) {
    // 連按：不關掉重開，原地輕震
    hud.classList.remove('is-in', 'is-out', 'is-swap');
    restart(hud, 'is-shake');
  } else if (changed) {
    hud.classList.remove('is-out');
    restart(hud, 'is-swap');
  }
  visible = true;
  hideTimer = setTimeout(hideHud, 1600);
}

function hideHud() {
  hud.classList.add('is-out');
  outTimer = setTimeout(() => {
    hud.className = 'hud';
    visible = false;
  }, 240);
}

document.addEventListener('copy', () => showHud(String(getSelection()).trim()));

// 點杯子也能複製（手機沒有鍵盤時特別好用）
$$('.cup-btn').forEach((btn) =>
  btn.addEventListener('click', async () => {
    const text = btn.dataset.copy;
    const target = btn.previousElementSibling;
    const range = document.createRange();
    range.selectNodeContents(target);
    const sel = getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      /* 不支援時仍顯示回饋 */
    }
    showHud(text);
  }),
);

// 點一下範例文字就整段選取，方便直接按 Ctrl+C
$$('.sample-text').forEach((el) =>
  el.addEventListener('click', () => {
    if (String(getSelection()).length) return;
    const range = document.createRange();
    range.selectNodeContents(el);
    getSelection().removeAllRanges();
    getSelection().addRange(range);
  }),
);

// 鍵帽跟著實體鍵盤按下
const modKeys = $$('kbd[data-mod]');
const cKeys = $$('kbd:not([data-mod])').filter((k) => k.textContent.trim() === 'C');
document.addEventListener('keydown', (e) => {
  if (e.metaKey || e.ctrlKey) modKeys.forEach((k) => k.classList.add('is-pressed'));
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'c') cKeys.forEach((k) => k.classList.add('is-pressed'));
});
document.addEventListener('keyup', () => [...modKeys, ...cKeys].forEach((k) => k.classList.remove('is-pressed')));

/* ---------------- 按壓傾斜卡片 ---------------- */

const tiltCard = $('#demo-card');
const glow = $('.tilt-glow', tiltCard);
let pressed = false;
function tilt(e) {
  const r = tiltCard.getBoundingClientRect();
  const x = e.clientX - r.left;
  const y = e.clientY - r.top;
  const ry = (x / r.width - 0.5) * 7;
  const rx = (0.5 - y / r.height) * 7;
  tiltCard.style.transition = 'transform 120ms ease-out';
  tiltCard.style.transform = `perspective(900px) rotateX(${rx.toFixed(2)}deg) rotateY(${ry.toFixed(2)}deg) scale(0.985)`;
  glow.style.background = `radial-gradient(240px circle at ${x}px ${y}px, rgba(200, 137, 77, 0.3), transparent 70%)`;
}
function release() {
  if (!pressed) return;
  pressed = false;
  tiltCard.style.transition = '';
  tiltCard.style.transform = '';
  glow.style.background = '';
}
if (!reduceMotion) {
  tiltCard.addEventListener('pointerdown', (e) => {
    if (e.target.closest('.sample-text')) return; // 不干擾文字選取
    pressed = true;
    tilt(e);
  });
  tiltCard.addEventListener('pointermove', (e) => pressed && tilt(e));
  ['pointerup', 'pointerleave', 'pointercancel'].forEach((t) => tiltCard.addEventListener(t, release));
}

/* ---------------- 導航：吸頂收縮 + 錨點高亮 ---------------- */

const nav = $('#nav');
const pill = $('#nav-pill');
const links = $$('.nav-links a');

function movePill(link) {
  if (!link) return pill.style.setProperty('--o', 0);
  pill.style.setProperty('--x', `${link.offsetLeft}px`);
  pill.style.setProperty('--w', `${link.offsetWidth}px`);
  pill.style.setProperty('--o', 1);
}
const cta = $('.nav-cta');
function activate(id) {
  const link = links.find((a) => a.dataset.spy === id);
  links.forEach((a) => a.classList.toggle('is-active', a === link));
  movePill(link);
  cta.classList.toggle('is-active', cta.dataset.spyCta === id);
}
const spy = new IntersectionObserver(
  (entries) => entries.forEach((e) => e.isIntersecting && activate(e.target.id)),
  { rootMargin: '-45% 0px -50% 0px' },
);
['intro', 'story', 'features', 'download'].forEach((id) => spy.observe(document.getElementById(id)));
addEventListener('scroll', () => nav.classList.toggle('is-compact', scrollY > 24), { passive: true });
addEventListener('resize', () => movePill($('.nav-links a.is-active')));

// 漢堡抽屜
const burger = $('#burger');
const drawer = $('#drawer');
const scrim = $('#drawer-scrim');
function setDrawer(open) {
  burger.setAttribute('aria-expanded', String(open));
  burger.setAttribute('aria-label', open ? T.menuClose : T.menuOpen);
  drawer.classList.toggle('is-open', open);
  scrim.hidden = !open;
}
burger.addEventListener('click', () => setDrawer(burger.getAttribute('aria-expanded') !== 'true'));
scrim.addEventListener('click', () => setDrawer(false));
$$('a', drawer).forEach((a) => a.addEventListener('click', () => setDrawer(false)));
document.addEventListener('keydown', (e) => e.key === 'Escape' && setDrawer(false));

/* ---------------- 功能：橫向手風琴 ---------------- */

const strips = $$('.strip');
strips.forEach((strip) =>
  $('.strip-head', strip).addEventListener('click', () => {
    strips.forEach((s) => {
      const on = s === strip;
      s.classList.toggle('is-active', on);
      $('.strip-head', s).setAttribute('aria-expanded', String(on));
    });
  }),
);

/* ---------------- 下載：托盤、Store ---------------- */

$$('.tray').forEach((tray) => {
  const btn = $('.tray-toggle', tray);
  btn.addEventListener('click', () => {
    const open = !tray.classList.contains('is-open');
    tray.classList.toggle('is-open', open);
    btn.setAttribute('aria-expanded', String(open));
  });
});

const download = $('#download');
const storeId = download.dataset.msStoreId?.trim();
if (storeId) {
  const store = $('#ms-store');
  store.href = `https://apps.microsoft.com/detail/${encodeURIComponent(storeId)}`;
  store.removeAttribute('aria-disabled');
  store.removeAttribute('tabindex');
  store.rel = 'noopener';
}

// 讀取 GitHub 最新版本，顯示版本號與檔案大小；還沒有發布時改連到 Releases 頁
(async () => {
  const repo = download.dataset.repo;
  const line = $('#release-line');
  try {
    const res = await fetch(`https://api.github.com/repos/${repo}/releases/latest`, {
      headers: { Accept: 'application/vnd.github+json' },
    });
    if (res.status === 404) {
      line.textContent = T.brewing;
      $$('[data-asset]').forEach((a) => {
        a.href = `https://github.com/${repo}/releases`;
        a.removeAttribute('download');
      });
      return;
    }
    if (!res.ok) return;
    const rel = await res.json();
    const date = new Date(rel.published_at).toLocaleDateString(T.locale);
    line.textContent = T.latest(rel.tag_name, date);
    const sizes = new Map(rel.assets.map((a) => [a.name, a.size]));
    $$('[data-asset]').forEach((a) => {
      const size = sizes.get(a.dataset.asset);
      if (!size) return;
      const tag = document.createElement('small');
      tag.className = 'num';
      tag.style.cssText = 'opacity:.7;font-weight:500';
      tag.textContent = ` · ${(size / 1048576).toFixed(1)} MB`;
      (a.querySelector('span') ?? a).append(tag);
    });
  } catch {
    /* 離線或被限流時保留預設文案 */
  }
})();

/* ---------------- 捲動進場 ---------------- */

/* ---------------- 介紹影片 ---------------- */

// 沒有 JS 時保留原生控制列；有 JS 時先只顯示封面與播放鈕，點了才載入、播放
const trailerFrame = $('#trailer-frame');
if (trailerFrame) {
  const video = $('#trailer-video');
  video.controls = false;
  $('#trailer-play').addEventListener('click', () => {
    trailerFrame.classList.add('is-playing');
    video.controls = true;
    video.play().catch(() => {});
  });
}

const revealTargets = $$('.section-head, .story-text, .menu-board, .accordion, .trailer-frame, .platform-wrap, .faq-list');
revealTargets.forEach((el) => el.classList.add('reveal'));
const revealer = new IntersectionObserver(
  (entries) =>
    entries.forEach((e) => {
      if (!e.isIntersecting) return;
      e.target.classList.add('is-visible');
      revealer.unobserve(e.target);
    }),
  { rootMargin: '0px 0px -10% 0px' },
);
revealTargets.forEach((el) => revealer.observe(el));

$('#year').textContent = new Date().getFullYear();
