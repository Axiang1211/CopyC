// 通知框文案與視覺組裝：HUD 與偏好設定（預覽、歷史）共用。
// 剪貼簿內容屬於不受信任的輸入：一律以 textContent 寫入，絕不拼接成 HTML。
import { locale, t } from './i18n.js';

const ICONS = {
  check:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M5.5 12.5l4.2 4.2L18.5 7.8"/></svg>',
  globe:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.4 2.5 3.6 5.3 3.6 8.5s-1.2 6-3.6 8.5c-2.4-2.5-3.6-5.3-3.6-8.5s1.2-6 3.6-8.5z"/></svg>',
  lock:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="10.5" width="14" height="9.5" rx="2.4"/><path d="M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5"/><circle cx="12" cy="15.2" r="1.1" fill="currentColor" stroke="none"/></svg>',
  folder:
    '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M3 7.2A2.2 2.2 0 0 1 5.2 5h3.9c.6 0 1.1.2 1.5.6l1.3 1.3h6.9A2.2 2.2 0 0 1 21 9.1v8.7a2.2 2.2 0 0 1-2.2 2.2H5.2A2.2 2.2 0 0 1 3 17.8z"/></svg>',
  image:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><rect x="3.5" y="5" width="17" height="14" rx="2.5"/><circle cx="9" cy="10" r="1.6"/><path d="M4 17l5-4.5 3.5 3 3-2.5 4.5 4"/></svg>',
};

/** 建立靜態圖示（只接受上方常數表中的名稱）。 */
export function icon(name, cls = '') {
  const span = document.createElement('span');
  span.className = `ico ico-${name} ${cls}`.trim();
  span.setAttribute('aria-hidden', 'true');
  span.innerHTML = ICONS[name] ?? '';
  return span;
}

const num = (n) => Number(n).toLocaleString(locale());

function colorKind(v) {
  if (v.startsWith('#')) return t('hud.colorKind', 'HEX');
  if (/^rgba?/i.test(v)) return t('hud.colorKind', 'RGB');
  return t('hud.colorKind', 'HSL');
}

/** 只放行 Rust 端產生的 PNG data URL。 */
export function safeImage(src) {
  return typeof src === 'string' && src.startsWith('data:image/png;base64,') ? src : null;
}

const seg = (t, cls) => ({ t, cls });

/**
 * 把後端 payload 轉成 { lead, title, detail, badge }。
 * title / detail 是片段陣列：{ t: 文字, cls?: 樣式 } 或 { dot: 顏色 }。
 */
/** `via`：內容是其他電腦即時同步過來的，那台電腦的名稱。 */
export function compose(p, count = 1, via = null) {
  let lead = { type: 'check' };
  let title;
  let detail = null;

  switch (p.kind) {
    case 'text':
      title = [seg(t('hud.copiedColon')), seg(t('hud.quote', `${p.preview}${p.truncated ? '…' : ''}`), 'quote')];
      detail = [seg(t('hud.textStats', num(p.chars), num(p.lines)), 'num')];
      break;
    case 'image':
      lead = { type: 'thumb', src: safeImage(p.thumb) };
      title = [seg(t('hud.image'))];
      detail = [seg(t('hud.imageSize', num(p.width), num(p.height), p.format), 'num')];
      break;
    case 'files':
      lead = { type: 'file', src: safeImage(p.icon), ext: p.ext, isDir: p.isDir };
      title = [seg(t('hud.fileTitle', p.isDir)), seg(p.first, 'strong')];
      detail =
        p.count > 1
          ? [seg(t('hud.filesMore', p.first, p.count), 'num')]
          : [seg(p.isDir ? t('hud.folder') : p.ext ? t('hud.fileExt', p.ext.toUpperCase()) : t('hud.file'))];
      break;
    case 'color':
      lead = { type: 'color', value: p.value };
      title = [seg(t('hud.colorTitle')), { dot: p.value }, seg(p.value, 'strong mono')];
      detail = [seg(colorKind(p.value))];
      break;
    case 'url':
      lead = { type: 'globe' };
      title = [seg(t('hud.url'))];
      detail = [seg(p.scheme, 'muted'), seg(p.domain, 'domain'), seg(p.rest, 'muted')];
      break;
    case 'sensitive':
      lead = { type: 'lock' };
      title = [seg(t('hud.secret')), seg('••••••••', 'mask')];
      detail = [seg(t('hud.secretDetail'))];
      break;
    case 'private':
      title = [seg(t('hud.copied'))];
      break;
    default:
      title = [seg(t('hud.generic', t(`label.${p.label ?? 'content'}`)))];
  }

  let badge = null;
  if (via) {
    // 不是在這台電腦複製的：標題說「已同步」，下一行說來自哪台、可以直接貼上
    const value = { text: title?.[1], color: title?.slice(1), url: [seg(p.domain, 'strong')] }[p.kind];
    title = value ? [seg(t('hud.syncedColon')), ...[value].flat()] : [seg(t('hud.synced'))];
    detail = [seg(t('hud.via', via))];
    return { lead, title, detail, badge };
  }
  if (count > 1) {
    title = [seg(t('hud.again'))];
    badge = t('hud.badge', count);
    detail = [seg(t('hud.relax'))];
  }
  return { lead, title, detail, badge };
}

/** 歷史紀錄用的單行摘要。 */
export function summary(p) {
  switch (p.kind) {
    case 'text':
      return [seg(`${p.preview}${p.truncated ? '…' : ''}`)];
    case 'image':
      return [seg(t('sum.image')), seg(`${num(p.width)} × ${num(p.height)}`, 'num muted')];
    case 'files':
      return p.count > 1 ? [seg(p.first), seg(t('sum.filesMore', p.count), 'muted')] : [seg(p.first)];
    case 'color':
      return [seg(p.value, 'mono')];
    case 'url':
      return [seg(p.domain, 'domain'), seg(p.rest, 'muted')];
    case 'sensitive':
      return [seg(t('sum.secret')), seg('••••••', 'mask')];
    case 'private':
      return [seg(t('sum.private'), 'muted')];
    default:
      return [seg(t(`label.${p.label ?? 'content'}`))];
  }
}

/** 把片段陣列寫進元素。 */
export function fill(el, parts) {
  el.replaceChildren();
  for (const part of parts ?? []) {
    if (part.dot) {
      const d = document.createElement('span');
      d.className = 'swatch-dot';
      d.style.background = part.dot;
      el.append(d);
      continue;
    }
    if (!part.t) continue;
    if (!part.cls) {
      el.append(document.createTextNode(part.t));
      continue;
    }
    const s = document.createElement('span');
    s.className = part.cls;
    s.textContent = part.t;
    el.append(s);
  }
}

/** 建立左側視覺：勾勾、縮圖、檔案圖示、色票、地球或鎖頭。 */
export function buildLead(lead, cls = 'lead') {
  const el = document.createElement('span');
  el.className = `${cls} lead-${lead.type}`;
  switch (lead.type) {
    case 'thumb':
      if (lead.src) {
        const img = document.createElement('img');
        img.src = lead.src;
        img.alt = '';
        el.append(img);
      } else {
        el.append(icon('image'));
      }
      break;
    case 'file':
      if (lead.src) {
        const img = document.createElement('img');
        img.src = lead.src;
        img.alt = '';
        el.append(img);
        el.classList.add('has-icon');
      } else if (lead.isDir) {
        el.append(icon('folder'));
        el.classList.add('is-dir');
      } else {
        const label = document.createElement('span');
        label.className = 'ext';
        label.textContent = (lead.ext || 'file').slice(0, 4).toUpperCase();
        el.append(label);
      }
      break;
    case 'color': {
      const sw = document.createElement('span');
      sw.className = 'swatch';
      sw.style.background = lead.value;
      el.append(sw);
      break;
    }
    case 'globe':
      el.append(icon('globe'));
      break;
    case 'lock':
      el.append(icon('lock', 'icon-optical-lock'));
      break;
    default:
      el.append(icon('check', 'icon-optical-check'));
  }
  return el;
}

export function timeAgo(ms) {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 10) return t('ago.now');
  if (s < 60) return t('ago.sec', s);
  if (s < 3600) return t('ago.min', Math.floor(s / 60));
  const d = new Date(ms);
  return new Intl.DateTimeFormat(locale(), { hour: '2-digit', minute: '2-digit' }).format(d);
}
