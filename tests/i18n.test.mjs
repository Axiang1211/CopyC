import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { DICTS, applyI18n, formatNumber, getLang, locale, normalizeLang, setLang, t } from '../src/shared/i18n.js';
import { compose, summary, timeAgo } from '../src/shared/compose.js';

const samples = {
  'hud.quote': ['<img onerror=alert(1)>'], 'hud.textStats': ['1', '2'],
  'hud.imageSize': ['1,920', '1,080', 'PNG'], 'hud.fileTitle': [false],
  'hud.filesMore': ['Report.docx', 3], 'hud.fileExt': ['PDF'], 'hud.colorKind': ['RGB'],
  'hud.generic': ['text'], 'hud.badge': [3], 'sum.filesMore': [3], 'ago.sec': [20], 'ago.min': [2],
  'tray.count': [1234], 'dial.valuetext': ['1.4'], 'content.count': [3, 5],
  'sample.aria': ['text'], 'suggest.add': ['App'], 'tag.remove': ['App'],
  'update.title': ['1.2.3'], 'update.desc': ['1.0.5'], 'about.version': ['1.0.5', 'Windows'],
  'ob.step': [2, 6], 'ob.tryLead': ['Ctrl+C'], 'ob.syncDone': ['a@b.com'], 'sync.device': ['MY-PC'],
  'sync.lastSync': ['2 min ago'], 'sync.lanPeers': [2], 'hud.via': ['MY-PC'], 'history.from': ['MY-PC'],
};

test('all seven dictionaries have complete, nonempty and renderable keys', () => {
  const keys = Object.keys(DICTS.en).sort();
  assert.equal(keys.length, 279);
  for (const [lang, dict] of Object.entries(DICTS)) {
    setLang(lang);
    assert.deepEqual(Object.keys(dict).sort(), keys, lang);
    for (const key of keys) {
      const text = t(key, ...(samples[key] ?? []));
      assert.equal(typeof text, 'string', `${lang} ${key}`);
      assert.ok(text.length, `${lang} ${key}`);
      assert.doesNotMatch(text, /undefined|NaN|\{\d+\}/, `${lang} ${key}`);
      if (typeof dict[key] === 'string') {
        for (const [, index] of dict[key].matchAll(/\{(\d+)\}/g)) {
          assert.ok(Number(index) < (samples[key]?.length ?? 0), `${lang} ${key}: placeholder ${index}`);
        }
      }
    }
    assert.notEqual(t('hud.fileTitle', false), t('hud.fileTitle', true));
    assert.ok(t('hud.imageSize', 1920, 1080, 'PNG').includes('(PNG)'));
    assert.ok(!t('hud.imageSize', 1920, 1080, '').includes('()'));
    assert.ok(t('hud.quote', '<script>').includes('<script>'));
    assert.ok(t('update.title', '1.2.3').includes('1.2.3'));
  }
});

test('system locales, scripts, regions, POSIX and fallback', () => {
  for (const [input, expected] of Object.entries({
    'zh-TW': 'zh-Hant', zh_HK: 'zh-Hant', 'zh-MO': 'zh-Hant', 'zh-Hant-CN': 'zh-Hant',
    'ZH_CN.UTF-8': 'zh-Hans', 'zh-SG': 'zh-Hans', 'zh-Hans-TW': 'zh-Hans',
    ja_JP: 'ja', 'ko-KR': 'ko', 'fr-CA': 'fr', 'ms-MY': 'ms', 'en-GB': 'en',
    'de-DE': 'en', zhx: 'en', '': 'en',
  })) assert.equal(normalizeLang(input), expected, input);
  setLang(null);
  assert.equal(getLang(), 'en');
});

test('missing translations preserve the existing Traditional Chinese fallback', () => {
  setLang('fr');
  const old = DICTS.fr['hud.copied'];
  delete DICTS.fr['hud.copied'];
  try { assert.equal(t('hud.copied'), DICTS['zh-Hant']['hud.copied']); }
  finally { DICTS.fr['hud.copied'] = old; }
  assert.equal(t('missing.key'), 'missing.key');
});

test('locale formatting and English singular/plural', () => {
  setLang('fr');
  assert.equal(locale(), 'fr-FR');
  assert.equal(formatNumber(1.4, {minimumFractionDigits: 1}), '1,4');
  assert.equal(formatNumber(1234), new Intl.NumberFormat('fr-FR').format(1234));
  for (const lang of Object.keys(DICTS)) {
    setLang(lang);
    assert.equal(formatNumber(1234), new Intl.NumberFormat(locale()).format(1234));
  }
  setLang('en');
  assert.equal(t('hud.textStats', '1', '1'), '1 character · 1 line');
  assert.equal(t('hud.textStats', '2', '2'), '2 characters · 2 lines');
});

test('all HUD kinds, repeat, summaries and relative/absolute times render in every language', () => {
  const payloads = [
    {kind:'text', preview:'<script>', chars:1234, lines:2},
    {kind:'image', width:1920, height:1080, format:'PNG'},
    {kind:'files', first:'Report.docx', count:3},
    {kind:'files', first:'Folder', count:1, isDir:true},
    {kind:'color', value:'#00ff00'}, {kind:'url', domain:'example.com', scheme:'https://',rest:'/a'},
    {kind:'sensitive'}, {kind:'private'}, {kind:'unknown',label:'content'},
  ];
  for (const lang of Object.keys(DICTS)) {
    setLang(lang);
    for (const p of payloads) {
      for (const n of [1, 3]) {
        const view = compose(p, n);
        assert.ok(view.title.length);
        assert.doesNotMatch(JSON.stringify(view), /undefined|NaN|\{\d+\}/);
      }
      assert.ok(summary(p).length);
    }
    for (const ago of [0, 20000, 120000, 7200000]) assert.ok(timeAgo(Date.now()-ago).length);
    assert.ok(!JSON.stringify(compose({kind:'private',preview:'SECRET'})).includes('SECRET'));
  }
});

test('HTML keys/options exist and all locale options match persistence and tray', () => {
  const html = readFileSync(new URL('../src/settings.html', import.meta.url), 'utf8');
  const hud = readFileSync(new URL('../src/hud.html', import.meta.url), 'utf8');
  for (const [,key] of (html+hud).matchAll(/data-i18n(?:-aria|-placeholder|-title)?="([^"]+)"/g)) {
    assert.ok(key in DICTS.en, key);
  }
  const options = JSON.parse(html.match(/data-setting="language" data-options='([^']+)'/)[1]);
  assert.equal(options.length, 8);
  const tray = readFileSync(new URL('../src-tauri/src/tray.rs', import.meta.url), 'utf8');
  for (const [value] of options) assert.ok(tray.includes(`("${value}", Language::`), value);
});

test('DOM application updates text, accessibility, placeholder, title and document language', () => {
  const elements = {
    '[data-i18n]': [{dataset:{i18n:'nav.general'}}],
    '[data-i18n-aria]': [{dataset:{i18nAria:'nav.aria'},setAttribute(k,v){this[k]=v;}}],
    '[data-i18n-placeholder]': [{dataset:{i18nPlaceholder:'privacy.placeholder'}}],
    '[data-i18n-title]': [{dataset:{i18nTitle:'general.title'}}],
  };
  globalThis.document = {documentElement:{},querySelectorAll:(selector)=>elements[selector]};
  try {
    for (const lang of Object.keys(DICTS)) {
      setLang(lang);
      applyI18n();
      assert.equal(document.documentElement.lang, lang);
      assert.equal(elements['[data-i18n]'][0].textContent, t('nav.general'));
      assert.equal(elements['[data-i18n-aria]'][0]['aria-label'], t('nav.aria'));
      assert.equal(elements['[data-i18n-placeholder]'][0].placeholder, t('privacy.placeholder'));
      assert.equal(elements['[data-i18n-title]'][0].dataset.title, t('general.title'));
    }
  } finally { delete globalThis.document; }
});

test('content synced from another computer says where it came from', () => {
  setLang('en');
  const text = compose({ kind: 'text', preview: 'hello', truncated: false, chars: 5, lines: 1 }, 1, 'MY-PC');
  assert.equal(text.title.map((s) => s.t ?? '').join(''), 'Synced:“hello”');
  assert.equal(text.detail[0].t, 'From MY-PC — ready to paste');
  assert.equal(text.badge, null);
  const color = compose({ kind: 'color', value: '#10B981' }, 1, 'MY-PC');
  assert.ok(color.title.some((s) => s.dot === '#10B981'));
  const url = compose({ kind: 'url', scheme: 'https://', domain: 'example.com', rest: '/a' }, 1, 'MY-PC');
  assert.equal(url.title[1].t, 'example.com');
  // 一般複製不受影響
  assert.equal(compose({ kind: 'private' }).title[0].t, t('hud.copied'));
});
