// 偏好設定視窗
import { buildLead, compose, fill, summary, timeAgo } from './shared/compose.js';
import { applyI18n, getLang, setLang, t } from './shared/i18n.js';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

let settings = null;
let overview = null;

const debounce = (fn, ms) => {
  let t = 0;
  return (...args) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...args), ms);
  };
};

function restart(el, cls) {
  el.classList.remove(cls);
  void el.offsetWidth;
  el.classList.add(cls);
}

const persist = debounce(async () => {
  settings = await invoke('update_settings', { settings });
  syncAll();
  refreshOverview(); // 語言可能改了，重新取得實際使用的語言
}, 120);

function set(key, value) {
  settings[key] = value;
  syncAll();
  persist();
}

/* ---------------- 平台與品牌 ---------------- */

const platformLabel = (p) => ({ macos: 'macOS', windows: 'Windows', linux: t('platform.linux') })[p] ?? p;

function applyPlatform(platform) {
  document.body.dataset.platform = platform;
  $('#hero-platform').textContent = platformLabel(platform);
  // Microsoft Store 版的開機啟動由 Windows 管理，App 內無法切換
  const store = overview.packaged;
  $('#autostart').hidden = store;
  // 商店版由 Microsoft Store 負責更新
  $('#updates-row').hidden = store;
  $('#autostart-hint').textContent = t(
    store ? 'general.autostartStore' : platform === 'macos' ? 'general.autostartMac' : 'general.autostartWin',
  );
  $('#about-version').textContent = t('about.version', overview.version, platformLabel(platform));
}

/** 套用語言：靜態文字、動態文字與所有元件重新繪製。 */
function applyLanguage(lang) {
  setLang(lang);
  applyI18n();
  document.title = t('doc.title');
  applyPlatform(overview.platform);
  const section = sections.find((sec) => sec.id === activeId);
  if (section) $('#page-title').textContent = section.dataset.title;
  renderHistory();
  renderUpdate();
  syncAll();
}

/* ---------------- 錨點導航 + 滾動收縮頂欄 ---------------- */

const scroller = $('#content-scroll');
const navItems = $$('.nav-item');
const sections = $$('.section');
let activeId = '';

function activate(id) {
  if (id === activeId) return;
  activeId = id;
  const item = navItems.find((n) => n.dataset.target === id);
  navItems.forEach((n) => n.classList.toggle('is-active', n === item));
  if (item) $('#nav-highlight').style.setProperty('--y', `${item.offsetTop}px`);
  const title = $('#page-title');
  title.textContent = sections.find((s) => s.id === id)?.dataset.title ?? '';
  restart(title, 'is-swap');
}

const spy = new IntersectionObserver(
  (entries) => {
    const hit = entries.filter((e) => e.isIntersecting).sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)[0];
    if (hit) activate(hit.target.id);
  },
  { root: scroller, rootMargin: '-30% 0px -60% 0px' },
);
sections.forEach((s) => spy.observe(s));

scroller.addEventListener(
  'scroll',
  () => {
    $('#topbar').classList.toggle('is-compact', scroller.scrollTop > 12);
    // 最後一段較短，捲到底時直接高亮
    if (scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 4) activate(sections.at(-1).id);
  },
  { passive: true },
);

navItems.forEach((item) =>
  item.addEventListener('click', (e) => {
    e.preventDefault();
    const target = document.getElementById(item.dataset.target);
    scroller.scrollTo({ top: target.offsetTop - 70, behavior: 'smooth' });
    activate(item.dataset.target);
  }),
);

/* ---------------- Split Button ---------------- */

const splitMenu = $('#split-menu');
const splitArrow = $('#split-arrow');

function setMenu(open) {
  splitMenu.classList.toggle('is-open', open);
  splitArrow.setAttribute('aria-expanded', String(open));
  if (open) splitMenu.querySelector('button')?.focus({ preventScroll: true });
}

$('#split-main').addEventListener('click', () => invoke('preview_hud', { kind: 'text' }));
splitArrow.addEventListener('click', () => setMenu(!splitMenu.classList.contains('is-open')));
splitMenu.addEventListener('click', (e) => {
  const kind = e.target.closest('[data-kind]')?.dataset.kind;
  if (!kind) return;
  setMenu(false);
  invoke('preview_hud', { kind });
});
splitMenu.addEventListener('keydown', (e) => {
  const items = $$('button', splitMenu);
  const i = items.indexOf(document.activeElement);
  if (e.key === 'ArrowDown') items[(i + 1) % items.length].focus();
  else if (e.key === 'ArrowUp') items[(i - 1 + items.length) % items.length].focus();
  else if (e.key === 'Escape') {
    setMenu(false);
    splitArrow.focus();
  } else return;
  e.preventDefault();
});

/* ---------------- 開關 ---------------- */

$$('input.switch[data-setting]').forEach((el) =>
  el.addEventListener('change', () => set(el.dataset.setting, el.checked)),
);

$('#status-toggle').addEventListener('click', () => set('paused', !settings.paused));

$('#autostart').addEventListener('change', async (e) => {
  e.target.checked = await invoke('set_autostart', { enabled: e.target.checked });
});

/* ---------------- 總覽：預覽、托盤、按壓傾斜 ---------------- */

const sampleText = () => ({ kind: 'text', preview: t('sample.textPreview'), truncated: true, chars: 142, lines: 3 });
const VARIANT = { top: 'pill', cursor: 'bubble' };

function mockHud(payload, variant, count = 1) {
  const hud = document.createElement('div');
  hud.className = 'hud is-shown';
  hud.dataset.variant = variant;
  hud.style.setProperty('--alpha', String(settings?.opacity ?? 0.96));
  const view = compose(payload, count);
  const main = document.createElement('div');
  main.className = 'main';
  const copy = document.createElement('div');
  copy.className = 'copy';
  const title = document.createElement('p');
  title.className = 'title';
  const titleText = document.createElement('span');
  fill(titleText, view.title);
  title.append(titleText);
  if (view.badge) {
    const badge = document.createElement('span');
    badge.className = 'badge num';
    badge.textContent = view.badge;
    title.append(badge);
  }
  const detail = document.createElement('p');
  detail.className = 'detail';
  fill(detail, view.detail);
  copy.append(title, detail);
  main.append(buildLead(view.lead), copy);
  hud.append(main);
  return hud;
}

let lastPreviewKey = '';
function renderHeroPreview() {
  const variant = VARIANT[settings.position] ?? 'card';
  const box = $('#hero-preview');
  const key = `${variant}|${settings.privacyMode}`;
  const payload = settings.privacyMode ? { kind: 'private' } : sampleText();
  const hud = mockHud(payload, variant);
  box.dataset.variant = variant;
  box.replaceChildren(hud);
  if (key !== lastPreviewKey) restart(hud, 'is-entering');
  lastPreviewKey = key;
}

const tray = $('#history-tray');
$('#tray-toggle').addEventListener('click', () => {
  const open = !tray.classList.contains('is-open');
  tray.classList.toggle('is-open', open);
  $('#tray-toggle').setAttribute('aria-expanded', String(open));
});
$('#clear-history').addEventListener('click', async () => {
  await invoke('clear_history');
  refreshOverview();
});

function renderHistory() {
  const items = overview.history;
  $('#tray-summary').textContent = items.length ? t('tray.count', items.length) : t('tray.none');
  $('#history-list').replaceChildren(
    ...items.map((item, i) => {
      const li = document.createElement('li');
      li.style.setProperty('--i', i);
      const text = document.createElement('span');
      text.className = 'text';
      fill(text, summary(item.payload));
      const meta = document.createElement('span');
      meta.className = 'meta';
      meta.textContent = [item.source, timeAgo(item.at)].filter(Boolean).join(' · ');
      li.append(buildLead(compose(item.payload).lead, 'mini'), text, meta);
      return li;
    }),
  );
}

function setStat(el, value) {
  const next = String(value);
  if (el.textContent !== next) {
    el.textContent = next;
    restart(el, 'bump');
  }
}

// 按壓傾斜卡片：朝按下的位置微微壓下，光斑聚攏到指尖
const hero = $('#hero');
const glow = $('#hero-glow');
let pressed = false;
function tilt(e) {
  const r = hero.getBoundingClientRect();
  const x = e.clientX - r.left;
  const y = e.clientY - r.top;
  const ry = (x / r.width - 0.5) * 6;
  const rx = (0.5 - y / r.height) * 6;
  hero.style.transition = 'transform 120ms ease-out';
  hero.style.transform = `perspective(900px) rotateX(${rx.toFixed(2)}deg) rotateY(${ry.toFixed(2)}deg) scale(0.985)`;
  glow.style.background = `radial-gradient(220px circle at ${x}px ${y}px, rgba(16, 185, 129, 0.34), transparent 70%)`;
}
function release() {
  if (!pressed) return;
  pressed = false;
  hero.style.transition = '';
  hero.style.transform = '';
  glow.style.background = '';
}
hero.addEventListener('pointerdown', (e) => {
  pressed = true;
  tilt(e);
});
hero.addEventListener('pointermove', (e) => pressed && tilt(e));
hero.addEventListener('pointerup', release);
hero.addEventListener('pointerleave', release);

/* ---------------- 顯示位置 ---------------- */

const spots = $$('.spot');
spots.forEach((spot) => spot.addEventListener('click', () => set('position', spot.dataset.position)));
$('#screen').addEventListener('keydown', (e) => {
  const i = spots.findIndex((s) => s.dataset.position === settings.position);
  const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
  if (!step) return;
  e.preventDefault();
  const next = spots[(i + step + spots.length) % spots.length];
  set('position', next.dataset.position);
  next.focus();
});

/* ---------------- Size Dial：停留秒數 ---------------- */

const DIAL_STEP = 48;
const DURATIONS = Array.from({ length: 23 }, (_, i) => 800 + i * 100);
const dial = $('#dial');
const dialTrack = $('#dial-track');
let dialIndex = -1;
let dialOffset = 0;

dialTrack.replaceChildren(
  ...DURATIONS.map((ms, i) => {
    const t = document.createElement('span');
    t.className = `tick${ms % 500 === 0 ? ' is-major' : ''}`;
    t.textContent = (ms / 1000).toFixed(1);
    t.dataset.index = i;
    return t;
  }),
);
const ticks = $$('.tick', dialTrack);

function paintDial(index, offset, snapping) {
  dialTrack.classList.toggle('is-snapping', snapping);
  dialTrack.style.setProperty('--offset', `${offset}px`);
  ticks.forEach((t, i) => t.classList.toggle('is-current', i === index));
  const seconds = (DURATIONS[index] / 1000).toFixed(1);
  $('#duration-label').textContent = seconds;
  dial.setAttribute('aria-valuenow', seconds);
  dial.setAttribute('aria-valuetext', t('dial.valuetext', seconds));
}
function dialTo(index, commit = true) {
  dialIndex = Math.max(0, Math.min(DURATIONS.length - 1, index));
  dialOffset = -dialIndex * DIAL_STEP;
  paintDial(dialIndex, dialOffset, true);
  if (commit && DURATIONS[dialIndex] !== settings.durationMs) set('durationMs', DURATIONS[dialIndex]);
}

let drag = null;
dial.addEventListener('pointerdown', (e) => {
  drag = { x: e.clientX, start: dialOffset, moved: false };
  dial.setPointerCapture(e.pointerId);
});
dial.addEventListener('pointermove', (e) => {
  if (!drag) return;
  const dx = e.clientX - drag.x;
  if (Math.abs(dx) > 3) drag.moved = true;
  const min = -(DURATIONS.length - 1) * DIAL_STEP;
  // 超出兩端時給一點橡皮筋阻力
  let offset = drag.start + dx;
  if (offset > 0) offset *= 0.35;
  if (offset < min) offset = min + (offset - min) * 0.35;
  dialOffset = offset;
  paintDial(Math.round(-Math.max(min, Math.min(0, offset)) / DIAL_STEP), offset, false);
});
dial.addEventListener('pointerup', (e) => {
  if (!drag) return;
  const tick = e.target.closest('.tick');
  const index = !drag.moved && tick ? Number(tick.dataset.index) : Math.round(-dialOffset / DIAL_STEP);
  drag = null;
  dialTo(index);
});
dial.addEventListener('pointercancel', () => {
  drag = null;
  dialTo(dialIndex, false);
});
let wheelAcc = 0;
const wheelCommit = debounce(() => dialTo(dialIndex), 220);
dial.addEventListener(
  'wheel',
  (e) => {
    e.preventDefault();
    wheelAcc += Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
    while (Math.abs(wheelAcc) >= 40) {
      const dir = Math.sign(wheelAcc);
      wheelAcc -= dir * 40;
      dialIndex = Math.max(0, Math.min(DURATIONS.length - 1, dialIndex + dir));
      dialOffset = -dialIndex * DIAL_STEP;
      paintDial(dialIndex, dialOffset, true);
    }
    wheelCommit();
  },
  { passive: false },
);
dial.addEventListener('keydown', (e) => {
  const map = { ArrowLeft: -1, ArrowDown: -1, ArrowRight: 1, ArrowUp: 1 };
  if (e.key in map) dialTo(dialIndex + map[e.key]);
  else if (e.key === 'Home') dialTo(0);
  else if (e.key === 'End') dialTo(DURATIONS.length - 1);
  else return;
  e.preventDefault();
});

/* ---------------- 不透明度 ---------------- */

const opacity = $('#opacity');
function paintOpacity(v) {
  opacity.style.setProperty('--p', `${((v - 60) / 40) * 100}%`);
  $('#opacity-value').textContent = `${v}%`;
}
opacity.addEventListener('input', () => {
  const v = Number(opacity.value);
  paintOpacity(v);
  $$('#hero-preview .hud').forEach((h) => h.style.setProperty('--alpha', String(v / 100)));
});
opacity.addEventListener('change', () => set('opacity', Number(opacity.value) / 100));

/* ---------------- 智慧內容：篩選標籤 + 範例 ---------------- */

const chips = $$('.chip');
chips.forEach((chip) =>
  chip.addEventListener('click', () => {
    const type = chip.dataset.type;
    settings.smartTypes = { ...settings.smartTypes, [type]: !settings.smartTypes[type] };
    set('smartTypes', settings.smartTypes);
    restart($('#chip-count'), 'bump');
  }),
);

function sampleThumb() {
  const c = document.createElement('canvas');
  c.width = 96;
  c.height = 64;
  const g = c.getContext('2d');
  const grad = g.createLinearGradient(0, 0, 96, 64);
  grad.addColorStop(0, '#5C9CE6');
  grad.addColorStop(1, '#8B80F9');
  g.fillStyle = grad;
  g.fillRect(0, 0, 96, 64);
  g.fillStyle = 'rgba(255,255,255,.85)';
  g.fillRect(12, 14, 46, 6);
  g.fillRect(12, 26, 30, 6);
  return c.toDataURL('image/png');
}

const THUMB = sampleThumb();
const samples = () => [
  { type: 'text', kind: 'text', name: t('sample.text'), rule: t('sample.textRule'), payload: sampleText() },
  {
    type: 'image',
    kind: 'image',
    name: t('sample.image'),
    rule: t('sample.imageRule'),
    payload: { kind: 'image', width: 1920, height: 1080, format: 'PNG', thumb: THUMB },
  },
  {
    type: 'files',
    kind: 'files',
    name: t('sample.files'),
    rule: t('sample.filesRule'),
    payload: { kind: 'files', first: t('sample.filesName'), count: 4, isDir: false, ext: 'docx', icon: null },
  },
  { type: 'color', kind: 'color', name: t('sample.color'), rule: t('sample.colorRule'), payload: { kind: 'color', value: '#10B981' } },
  {
    type: 'url',
    kind: 'url',
    name: t('sample.url'),
    rule: t('sample.urlRule'),
    payload: { kind: 'url', scheme: 'https://', domain: 'example.com', rest: '/products/…' },
  },
  {
    type: null,
    kind: 'sensitive',
    name: t('sample.secret'),
    rule: t('sample.secretRule'),
    payload: { kind: 'sensitive', reason: 'entropy' },
  },
];

function renderGallery() {
  $('#gallery').replaceChildren(
    ...samples().map((s) => {
      const off = s.type && !settings.smartTypes[s.type];
      const payload = off ? { kind: 'generic', label: s.type } : s.payload;
      const card = document.createElement('button');
      card.type = 'button';
      card.className = `sample${off ? ' is-off' : ''}`;
      card.setAttribute('aria-label', t('sample.aria', s.name));
      const screen = document.createElement('span');
      screen.className = 'sample-screen';
      screen.append(mockHud(payload, 'card'));
      const caption = document.createElement('span');
      caption.className = 'sample-caption';
      const name = document.createElement('strong');
      name.textContent = s.name;
      const rule = document.createElement('span');
      rule.textContent = off ? t('sample.off') : s.rule;
      caption.append(name, rule);
      card.append(screen, caption);
      card.addEventListener('click', () => invoke('preview_hud', { kind: s.kind }));
      return card;
    }),
  );
}

/* ---------------- 黑名單：可刪除標籤 + 聯想浮層 ---------------- */

const KNOWN_APPS = [
  '1Password',
  'Bitwarden',
  'KeePass',
  'KeePassXC',
  'LastPass',
  'Dashlane',
  'Keychain Access',
  'Enpass',
  'NordPass',
  'Proton Pass',
  'RoboForm',
  'Keeper',
];
const AVATAR_COLORS = ['#10B981', '#E5A93C', '#E05A47', '#5C9CE6', '#8B80F9'];
const avatarColor = (name) => AVATAR_COLORS[[...name].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7) % AVATAR_COLORS.length];

const tagsBox = $('#tags');
const tagsList = $('#tags-list');
const tagsInput = $('#tags-input');
const suggest = $('#suggest');
let suggestions = [];
let suggestIndex = 0;

function avatar(name) {
  const a = document.createElement('span');
  a.className = 'tag-avatar';
  a.style.background = avatarColor(name);
  a.textContent = [...name.trim()][0]?.toUpperCase() ?? '?';
  return a;
}

function tagEl(name, isNew) {
  const tag = document.createElement('span');
  tag.className = `tag${isNew ? ' is-new' : ''}`;
  tag.dataset.name = name;
  const label = document.createElement('span');
  label.className = 'tag-name';
  label.textContent = name;
  const remove = document.createElement('button');
  remove.type = 'button';
  remove.className = 'tag-remove';
  remove.setAttribute('aria-label', t('tag.remove', name));
  remove.innerHTML = '<svg viewBox="0 0 24 24"><path d="M7 7l10 10M17 7L7 17"/></svg>';
  remove.addEventListener('click', () => removeTag(name));
  tag.append(avatar(name), label, remove);
  return tag;
}

function renderTags(added) {
  const current = $$('.tag', tagsList).map((t) => t.dataset.name);
  if (!added && current.join('\n') === settings.blacklist.join('\n')) return;
  tagsList.replaceChildren(...settings.blacklist.map((n) => tagEl(n, n === added)));
}

// 刪除時先縮成小圓點，其餘標籤平滑補位（FLIP），輸入框高度跟著收縮
function removeTag(name) {
  const el = $$('.tag', tagsList).find((t) => t.dataset.name === name);
  if (!el) return;
  el.classList.add('is-removing');
  setTimeout(() => {
    const others = $$('.tag', tagsList).filter((t) => t !== el);
    const before = new Map(others.map((t) => [t, t.getBoundingClientRect()]));
    const h0 = tagsBox.offsetHeight;
    el.remove();
    const h1 = tagsBox.offsetHeight;
    others.forEach((t) => {
      const a = before.get(t);
      const b = t.getBoundingClientRect();
      const dx = a.left - b.left;
      const dy = a.top - b.top;
      if (!dx && !dy) return;
      t.style.transition = 'none';
      t.style.transform = `translate(${dx}px, ${dy}px)`;
      void t.offsetWidth;
      t.style.transition = '';
      t.style.transform = '';
    });
    if (h0 !== h1) {
      tagsBox.style.height = `${h0}px`;
      void tagsBox.offsetHeight;
      tagsBox.style.height = `${h1}px`;
      setTimeout(() => (tagsBox.style.height = ''), 280);
    }
    set(
      'blacklist',
      settings.blacklist.filter((n) => n !== name),
    );
  }, 200);
}

function addTag(name) {
  const clean = name.trim();
  if (!clean) return;
  tagsInput.value = '';
  closeSuggest();
  if (settings.blacklist.some((n) => n.toLowerCase() === clean.toLowerCase())) return;
  settings.blacklist = [...settings.blacklist, clean];
  renderTags(clean);
  set('blacklist', settings.blacklist);
}

function highlight(name, query) {
  const frag = document.createDocumentFragment();
  const i = query ? name.toLowerCase().indexOf(query.toLowerCase()) : -1;
  if (i < 0) {
    frag.append(name);
    return frag;
  }
  const mark = document.createElement('mark');
  mark.textContent = name.slice(i, i + query.length);
  frag.append(name.slice(0, i), mark, name.slice(i + query.length));
  return frag;
}

function openSuggest() {
  const query = tagsInput.value.trim();
  const recent = overview?.recentApps ?? [];
  const taken = new Set(settings.blacklist.map((n) => n.toLowerCase()));
  const seen = new Set();
  suggestions = [...recent.map((n) => [n, t('suggest.recent')]), ...KNOWN_APPS.map((n) => [n, t('suggest.pm')])]
    .filter(([n]) => {
      const k = n.toLowerCase();
      if (taken.has(k) || seen.has(k)) return false;
      seen.add(k);
      return !query || k.includes(query.toLowerCase());
    })
    .slice(0, 6);
  suggestIndex = 0;

  if (!suggestions.length && !query) return closeSuggest();
  suggest.replaceChildren(
    ...(suggestions.length
      ? suggestions.map(([name, hint], i) => {
          const b = document.createElement('button');
          b.type = 'button';
          b.className = `suggest-item${i === 0 ? ' is-active' : ''}`;
          b.setAttribute('role', 'option');
          const label = document.createElement('span');
          label.append(highlight(name, query));
          const h = document.createElement('span');
          h.className = 'hint';
          h.textContent = hint;
          b.append(avatar(name), label, h);
          b.addEventListener('pointerdown', (e) => {
            e.preventDefault();
            addTag(name);
          });
          return b;
        })
      : [Object.assign(document.createElement('p'), { className: 'suggest-empty', textContent: t('suggest.add', query) })]),
  );
  // 浮層原地長在輸入游標旁
  suggest.style.left = `${Math.min(tagsInput.offsetLeft, tagsBox.clientWidth - 290)}px`;
  suggest.style.top = `${tagsInput.offsetTop + tagsInput.offsetHeight + 6}px`;
  suggest.classList.add('is-open');
}

function closeSuggest() {
  suggest.classList.remove('is-open');
  suggestions = [];
}

function moveSuggest(dir) {
  if (!suggestions.length) return;
  suggestIndex = (suggestIndex + dir + suggestions.length) % suggestions.length;
  $$('.suggest-item', suggest).forEach((b, i) => b.classList.toggle('is-active', i === suggestIndex));
}

tagsBox.addEventListener('click', (e) => {
  if (e.target === tagsBox) tagsInput.focus();
});
tagsInput.addEventListener('focus', openSuggest);
tagsInput.addEventListener('input', openSuggest);
tagsInput.addEventListener('blur', () => setTimeout(closeSuggest, 120));
tagsInput.addEventListener('keydown', (e) => {
  if (e.key === 'ArrowDown') moveSuggest(1);
  else if (e.key === 'ArrowUp') moveSuggest(-1);
  else if (e.key === 'Enter') addTag(suggestions[suggestIndex]?.[0] ?? tagsInput.value);
  else if (e.key === 'Escape') closeSuggest();
  else if (e.key === 'Backspace' && !tagsInput.value && settings.blacklist.length) removeTag(settings.blacklist.at(-1));
  else return;
  e.preventDefault();
});

/* ---------------- Select 單選下拉 ---------------- */

const CHEVRON = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M7 10l5 5 5-5"/></svg>';
const CHECK = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5.5 12.5l4.2 4.2L18.5 7.8"/></svg>';
const selects = [];

function makeSelect(root) {
  const key = root.dataset.setting;
  const options = JSON.parse(root.dataset.options);
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'select-btn';
  btn.setAttribute('aria-haspopup', 'listbox');
  btn.setAttribute('aria-expanded', 'false');
  const label = document.createElement('span');
  btn.append(label);
  btn.insertAdjacentHTML('beforeend', CHEVRON);
  const panel = document.createElement('div');
  panel.className = 'select-panel';
  panel.setAttribute('role', 'listbox');
  const optionEls = options.map(([value, text]) => {
    const o = document.createElement('button');
    o.type = 'button';
    o.className = 'select-option';
    o.setAttribute('role', 'option');
    o.dataset.value = value;
    const txt = document.createElement('span');
    txt.className = 'select-text';
    o.append(txt);
    o.insertAdjacentHTML('beforeend', CHECK);
    o.addEventListener('click', () => {
      toggle(false);
      btn.focus();
      if (settings[key] !== value) set(key, value);
    });
    return o;
  });
  panel.append(...optionEls);
  root.append(btn, panel);

  let active = 0;
  function toggle(open) {
    root.classList.toggle('is-open', open);
    btn.setAttribute('aria-expanded', String(open));
    if (open) {
      active = Math.max(0, options.findIndex(([v]) => v === settings[key]));
      paintActive();
      // 下方空間不足時改為向上展開
      const below = window.innerHeight - btn.getBoundingClientRect().bottom;
      root.classList.toggle('is-up', below < panel.offsetHeight + 16);
    }
  }
  function paintActive() {
    optionEls.forEach((o, i) => o.classList.toggle('is-active', i === active));
  }
  btn.addEventListener('click', () => toggle(!root.classList.contains('is-open')));
  root.addEventListener('keydown', (e) => {
    const open = root.classList.contains('is-open');
    if (e.key === 'Escape' && open) toggle(false);
    else if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && !open) toggle(true);
    else if (e.key === 'ArrowDown') {
      active = (active + 1) % options.length;
      paintActive();
    } else if (e.key === 'ArrowUp') {
      active = (active - 1 + options.length) % options.length;
      paintActive();
    } else if (e.key === 'Enter' && open) optionEls[active].click();
    else return;
    e.preventDefault();
  });

  const sync = () => {
    const current = options.find(([v]) => v === settings[key]) ?? options[0];
    label.textContent = t(current[1]);
    optionEls.forEach((o, i) => ($('.select-text', o).textContent = t(options[i][1])));
    optionEls.forEach((o) => {
      const on = o.dataset.value === current[0];
      o.classList.toggle('is-selected', on);
      o.setAttribute('aria-selected', String(on));
    });
  };
  selects.push({ root, sync, close: () => toggle(false) });
}
$$('.select[data-setting]').forEach(makeSelect);

document.addEventListener('pointerdown', (e) => {
  if (!e.target.closest('#split')) setMenu(false);
  selects.forEach((s) => !s.root.contains(e.target) && s.close());
});

/* ---------------- 重設 ---------------- */

const resetBtn = $('#reset');
let resetArmed = 0;
resetBtn.addEventListener('click', async () => {
  if (!resetArmed) {
    resetBtn.textContent = t('reset.confirm');
    resetArmed = setTimeout(() => {
      resetArmed = 0;
      resetBtn.textContent = t('reset.btn');
    }, 2600);
    return;
  }
  clearTimeout(resetArmed);
  resetArmed = 0;
  resetBtn.textContent = t('reset.done');
  settings = await invoke('update_settings', { settings: {} });
  syncAll();
  setTimeout(() => (resetBtn.textContent = t('reset.btn')), 1400);
});

/* ---------------- 同步 ---------------- */

function syncAll() {
  if (!settings) return;
  $$('input.switch[data-setting]').forEach((el) => (el.checked = Boolean(settings[el.dataset.setting])));

  const status = $('#status-toggle');
  status.classList.toggle('is-paused', settings.paused);
  status.setAttribute('aria-checked', String(!settings.paused));
  $('#status-text').textContent = t(settings.paused ? 'status.paused' : 'status.on');
  $('#status-hint').textContent = t(settings.paused ? 'status.hintResume' : 'status.hintPause');
  $('#hero').classList.toggle('is-paused', settings.paused);
  $('#live-text').textContent = t(settings.paused ? 'hero.livePaused' : 'hero.live');

  spots.forEach((s) => {
    const on = s.dataset.position === settings.position;
    s.setAttribute('aria-checked', String(on));
    s.tabIndex = on ? 0 : -1;
  });
  $('#position-caption').textContent = t(`cap.${settings.position}`);

  if (!drag) {
    const i = DURATIONS.indexOf(Math.round(settings.durationMs / 100) * 100);
    if (i >= 0 && i !== dialIndex) dialTo(i, false);
    else paintDial(dialIndex, dialOffset, true);
  }

  const op = Math.round(settings.opacity * 100);
  if (document.activeElement !== opacity) {
    opacity.value = op;
    paintOpacity(op);
  }

  chips.forEach((c) => c.setAttribute('aria-pressed', String(Boolean(settings.smartTypes[c.dataset.type]))));
  const on = Object.values(settings.smartTypes).filter(Boolean).length;
  $('#chip-count').textContent = t('content.count', on, chips.length);

  renderTags();
  selects.forEach((s) => s.sync());
  renderHeroPreview();
  renderGallery();
}

/* ---------------- 新版本提示 ---------------- */

let updateDismissed = '';

function renderUpdate() {
  const version = overview.update;
  const card = $('#update-card');
  card.hidden = !version || version === updateDismissed;
  if (card.hidden) return;
  $('#update-title').textContent = t('update.title', version);
  $('#update-desc').textContent = t('update.desc', overview.version);
}

$('#update-go').addEventListener('click', () => invoke('open_download'));
$('#update-later').addEventListener('click', () => {
  updateDismissed = overview.update;
  renderUpdate();
});

async function refreshOverview() {
  overview = await invoke('get_overview');
  if (overview.lang !== getLang()) applyLanguage(overview.lang);
  setStat($('#stat-copies'), overview.stats.copies);
  setStat($('#stat-confirms'), overview.stats.confirms);
  $('#autostart').checked = overview.autostart;
  renderHistory();
  renderUpdate();
}

async function init() {
  [settings, overview] = await Promise.all([invoke('get_settings'), invoke('get_overview')]);
  setLang(overview.lang);
  applyI18n();
  document.title = t('doc.title');
  applyPlatform(overview.platform);
  $('#version').textContent = `v${overview.version}`;
  $('#stat-copies').textContent = overview.stats.copies;
  $('#stat-confirms').textContent = overview.stats.confirms;
  $('#autostart').checked = overview.autostart;
  renderHistory();
  renderUpdate();
  syncAll();
  activate('overview');

  listen('settings:changed', (e) => {
    settings = e.payload;
    syncAll();
    refreshOverview();
  });
  listen('overview:changed', debounce(refreshOverview, 150));
  setInterval(() => overview && renderHistory(), 30_000);
}

// 偏好設定的快捷鍵：⌘/Ctrl + W 關閉
document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'w') window.__TAURI__.window.getCurrentWindow().close();
});

init();
