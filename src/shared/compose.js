// 通知框文案與視覺組裝：HUD 與偏好設定（預覽、歷史）共用。
// 剪貼簿內容屬於不受信任的輸入：一律以 textContent 寫入，絕不拼接成 HTML。

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

const num = (n) => Number(n).toLocaleString('zh-TW');

function colorKind(v) {
  if (v.startsWith('#')) return 'HEX 色碼';
  if (/^rgba?/i.test(v)) return 'RGB 色碼';
  return 'HSL 色碼';
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
export function compose(p, count = 1) {
  let lead = { type: 'check' };
  let title;
  let detail = null;

  switch (p.kind) {
    case 'text':
      title = [seg('你已成功複製：'), seg(`「${p.preview}${p.truncated ? '…' : ''}」`, 'quote')];
      detail = [seg(`共 ${num(p.chars)} 字元 · ${num(p.lines)} 行`, 'num')];
      break;
    case 'image':
      lead = { type: 'thumb', src: safeImage(p.thumb) };
      title = [seg('你已成功複製圖片')];
      detail = [seg(`尺寸：${num(p.width)} × ${num(p.height)} px${p.format ? ` (${p.format})` : ''}`, 'num')];
      break;
    case 'files':
      lead = { type: 'file', src: safeImage(p.icon), ext: p.ext, isDir: p.isDir };
      title = [seg(`你已成功複製${p.isDir ? '資料夾' : '檔案'}：`), seg(p.first, 'strong')];
      detail =
        p.count > 1
          ? [seg(`（${p.first} 等共 ${num(p.count)} 個項目）`, 'num')]
          : [seg(p.isDir ? '資料夾' : p.ext ? `${p.ext.toUpperCase()} 檔案` : '檔案')];
      break;
    case 'color':
      lead = { type: 'color', value: p.value };
      title = [seg('你已成功複製色碼：'), { dot: p.value }, seg(p.value, 'strong mono')];
      detail = [seg(colorKind(p.value))];
      break;
    case 'url':
      lead = { type: 'globe' };
      title = [seg('你已成功複製連結')];
      detail = [seg(p.scheme, 'muted'), seg(p.domain, 'domain'), seg(p.rest, 'muted')];
      break;
    case 'sensitive':
      lead = { type: 'lock' };
      title = [seg('你已成功複製敏感內容'), seg('••••••••', 'mask')];
      detail = [seg('（已啟動隱私安全遮罩保護）')];
      break;
    case 'private':
      title = [seg('你已成功複製')];
      break;
    default:
      title = [seg(`你已成功複製${p.label ?? ''}`)];
  }

  let badge = null;
  if (count > 1) {
    title = [seg('你已成功複製！')];
    badge = `已確認 ×${count} 次`;
    detail = [seg('放心吧！內容已經安全躺在剪貼簿裡了')];
  }
  return { lead, title, detail, badge };
}

/** 歷史紀錄用的單行摘要。 */
export function summary(p) {
  switch (p.kind) {
    case 'text':
      return [seg(`${p.preview}${p.truncated ? '…' : ''}`)];
    case 'image':
      return [seg('圖片 '), seg(`${p.width} × ${p.height}`, 'num muted')];
    case 'files':
      return p.count > 1 ? [seg(p.first), seg(` 等 ${p.count} 項`, 'muted')] : [seg(p.first)];
    case 'color':
      return [seg(p.value, 'mono')];
    case 'url':
      return [seg(p.domain, 'domain'), seg(p.rest, 'muted')];
    case 'sensitive':
      return [seg('敏感內容 '), seg('••••••', 'mask')];
    case 'private':
      return [seg('已隱藏（隱私打碼模式）', 'muted')];
    default:
      return [seg(p.label ?? '內容')];
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
  if (s < 10) return '剛剛';
  if (s < 60) return `${s} 秒前`;
  if (s < 3600) return `${Math.floor(s / 60)} 分鐘前`;
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}
