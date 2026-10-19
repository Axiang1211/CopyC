// CopyC 官網的 Worker：其他檔案都直接由 Workers 靜態資產回應，
// 只有 /assets/video/* 會先經過這裡（見 wrangler.jsonc 的 run_worker_first）。
// 靜態資產不支援 Range 請求，而 iOS / macOS 的 Safari 播放 MP4 一定要分段下載，所以在這裡補上 206。

const RANGE = /^bytes=(\d*)-(\d*)$/;

export default {
  async fetch(request, env) {
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
