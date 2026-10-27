// CopyC 官網的 Worker：其他檔案都直接由 Workers 靜態資產回應，
// 只有 /assets/video/* 與 /sync/* 會先經過這裡（見 wrangler.jsonc 的 run_worker_first）。
// - /assets/video/*：靜態資產不支援 Range 請求，而 iOS / macOS 的 Safari 播放 MP4 一定要分段下載，所以在這裡補上 206。
// - /sync/ws：App 的即時同步中繼（見 relay.js）。

import { googleKeys, verifyIdToken } from './google-jwt.js';

export { Relay } from './relay.js';

const RANGE = /^bytes=(\d*)-(\d*)$/;

/** 驗證 Google 登入後，把 WebSocket 交給這個帳號的房間。 */
async function sync(request, env) {
  if (new URL(request.url).pathname !== '/sync/ws') return new Response('Not found', { status: 404 });
  if (request.headers.get('Upgrade') !== 'websocket') return new Response('Expected WebSocket', { status: 426 });
  // GOOGLE_CLIENT_ID 在 Cloudflare 主控台設定；還沒設定時回 503，App 會很久之後再試
  if (!env.GOOGLE_CLIENT_ID || !env.RELAY) return new Response('Sync is not configured', { status: 503 });
  const auth = request.headers.get('Authorization') ?? '';
  const token = auth.startsWith('Bearer ') ? auth.slice(7) : '';
  let sub;
  try {
    sub = await verifyIdToken(token, { audience: env.GOOGLE_CLIENT_ID, keys: await googleKeys() });
  } catch {
    return new Response('Upstream error', { status: 502 });
  }
  if (!sub) return new Response('Unauthorized', { status: 401 });
  const room = env.RELAY.get(env.RELAY.idFromName(sub));
  // 只轉交升級請求本身，token 不會傳進房間
  return room.fetch(new Request('https://relay.internal/ws', { headers: { Upgrade: 'websocket' } }));
}

export default {
  async fetch(request, env) {
    if (new URL(request.url).pathname.startsWith('/sync/')) return sync(request, env);
    const res = await env.ASSETS.fetch(request);
    const range = request.headers.get('Range');
    if (!range || res.status !== 200) return res;

    const match = RANGE.exec(range.trim());
    if (!match || (match[1] === '' && match[2] === '')) return res;

    const body = await res.arrayBuffer();
    const size = body.byteLength;
    let start;
    let end;
    if (match[1] === '') {
      // bytes=-N：最後 N 個位元組
      start = Math.max(0, size - Number(match[2]));
      end = size - 1;
    } else {
      start = Number(match[1]);
      end = match[2] === '' ? size - 1 : Math.min(Number(match[2]), size - 1);
    }
    if (start >= size || start > end) {
      return new Response(null, { status: 416, headers: { 'Content-Range': `bytes */${size}` } });
    }

    const headers = new Headers(res.headers);
    headers.set('Accept-Ranges', 'bytes');
    headers.set('Content-Range', `bytes ${start}-${end}/${size}`);
    headers.set('Content-Length', String(end - start + 1));
    return new Response(body.slice(start, end + 1), { status: 206, headers });
  },
};
