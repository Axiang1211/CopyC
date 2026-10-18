// npm run build 的入口。
// Cloudflare Pages 會自動執行 `npm run build`：官網是純靜態檔案（website/），不需要任何建置，
// 也沒有 Rust 工具鏈，所以在 Cloudflare 上直接結束；其他環境照常建置桌面 App（tauri build）。
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';

if (process.env.CF_PAGES === '1') {
  if (!existsSync('website/index.html')) {
    console.error('找不到 website/index.html');
    process.exit(1);
  }
  console.log('Cloudflare Pages：官網為靜態檔案，略過桌面 App 建置。');
  console.log('請確認 Build output directory 設為 website');
  process.exit(0);
}

const r = spawnSync('npx', ['tauri', 'build', ...process.argv.slice(2)], { stdio: 'inherit', shell: process.platform === 'win32' });
process.exit(r.status ?? 1);
