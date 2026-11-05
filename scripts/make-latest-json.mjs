// 產生自動更新用的 latest.json（附在 GitHub Release，App 從 releases/latest/download/latest.json 讀取）。
// 只列出有簽章檔（.sig）的平台；一個都沒有時不產生，App 會退回「提示到官網下載」。
//
//   node scripts/make-latest-json.mjs <tag> <artifacts 資料夾> <owner/repo>
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const [tag, dir, repo] = process.argv.slice(2);
if (!tag || !dir || !repo) {
  console.error('用法：node scripts/make-latest-json.mjs <tag> <dir> <owner/repo>');
  process.exit(1);
}
const url = (file) => `https://github.com/${repo}/releases/download/${tag}/${file}`;
// 更新檔 → 適用的平台（macOS 是 Universal，同一個檔案給兩種晶片）
const FILES = {
  'CopyC-windows-x64-setup.exe': ['windows-x86_64'],
  'CopyC-windows-arm64-setup.exe': ['windows-aarch64'],
  'CopyC-macos-universal.app.tar.gz': ['darwin-aarch64', 'darwin-x86_64'],
};

const platforms = {};
for (const [file, targets] of Object.entries(FILES)) {
  const sig = join(dir, `${file}.sig`);
  if (!existsSync(sig) || !existsSync(join(dir, file))) continue;
  for (const target of targets) platforms[target] = { signature: readFileSync(sig, 'utf8').trim(), url: url(file) };
}
if (!Object.keys(platforms).length) {
  console.log('沒有簽章檔（未設定 TAURI_SIGNING_PRIVATE_KEY），不產生 latest.json');
  process.exit(0);
}
const latest = {
  version: tag.replace(/^v/, ''),
  notes: `https://github.com/${repo}/releases/tag/${tag}`,
  pub_date: new Date().toISOString(),
  platforms,
};
writeFileSync(join(dir, 'latest.json'), `${JSON.stringify(latest, null, 2)}\n`);
console.log(`latest.json：${Object.keys(platforms).join(', ')}`);
