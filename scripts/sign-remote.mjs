// 簽署遠端資料：讀取 config/remote.json，檢查格式，加上發佈時間（seq），用 Ed25519 私鑰簽章後
// 寫到 website/config/v1.json（官網部署後 App 就會下載）。由 .github/workflows/remote-data.yml 執行。
//
//   COPYC_REMOTE_SIGNING_KEY=<PKCS#8 DER 的 base64> node scripts/sign-remote.mjs
//   node scripts/sign-remote.mjs --in <檔案> --out <檔案> --key-file <檔案>   # 產生測試檔用
import { createPrivateKey, sign } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';

const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i >= 0 ? process.argv[i + 1] : fallback;
};
const input = arg('in', 'config/remote.json');
const output = arg('out', 'website/config/v1.json');
const keyFile = arg('key-file');
const keyB64 = (keyFile ? readFileSync(keyFile, 'utf8') : process.env.COPYC_REMOTE_SIGNING_KEY ?? '').trim();
if (!keyB64) {
  console.error('缺少簽章私鑰：請在 GitHub → Settings → Secrets and variables → Actions 新增 COPYC_REMOTE_SIGNING_KEY。');
  process.exit(1);
}

const LANGS = ['zh-Hant', 'zh-Hans', 'en', 'ja', 'ko', 'fr', 'ms'];
const fail = (msg) => {
  console.error(`config/remote.json 格式錯誤：${msg}`);
  process.exit(1);
};
const isText = (v) => typeof v === 'string' && v.trim() !== '';
const isTextMap = (m) => m && typeof m === 'object' && Object.entries(m).every(([k, v]) => LANGS.includes(k) && isText(v));

const remote = JSON.parse(readFileSync(input, 'utf8'));
delete remote.$comment;
for (const [i, n] of (remote.notices ?? []).entries()) {
  if (!isText(n.id)) fail(`notices[${i}] 需要 id`);
  if (!isTextMap(n.title) || !Object.keys(n.title).length) fail(`notices[${i}].title 需要至少一種語言（${LANGS.join(', ')}）`);
  if (n.body && !isTextMap(n.body)) fail(`notices[${i}].body 的語言代碼不對`);
  if (n.url && !/^https:\/\/\S+$/.test(n.url)) fail(`notices[${i}].url 必須是 https 網址`);
  if (n.until !== undefined && typeof n.until !== 'number') fail(`notices[${i}].until 必須是毫秒數字`);
}
if (remote.blacklist && !(Array.isArray(remote.blacklist) && remote.blacklist.every(isText))) fail('blacklist 必須是 App 名稱的清單');
for (const [lang, map] of Object.entries(remote.strings ?? {})) {
  if (!LANGS.includes(lang)) fail(`strings 的語言代碼「${lang}」不對`);
  if (!Object.values(map).every(isText)) fail(`strings.${lang} 的文字不能是空的`);
}
for (const [k, v] of Object.entries(remote.features ?? {})) {
  if (!['live', 'lan', 'backup'].includes(k) || typeof v !== 'boolean') fail(`features.${k} 必須是 true 或 false`);
}

remote.seq = Math.floor(Date.now() / 1000);
const data = Buffer.from(JSON.stringify(remote));
const key = createPrivateKey({ key: Buffer.from(keyB64, 'base64'), format: 'der', type: 'pkcs8' });
const sig = sign(null, data, key);
writeFileSync(output, `${JSON.stringify({ data: data.toString('base64'), sig: sig.toString('base64') })}\n`);
console.log(`已簽署 seq ${remote.seq} → ${output}`);
