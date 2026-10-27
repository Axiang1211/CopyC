// 即時同步的中繼：每個 Google 帳號一個 Durable Object（房間），同一帳號的裝置連進同一間。
// 收到的訊息原封不動轉給房間裡的其他裝置；內容在裝置上就加密了，這裡解不開，也不保存任何東西。
// 使用 WebSocket Hibernation：沒有訊息時物件會休眠，"ping" 由執行環境自動回 "pong"，不會喚醒它。

import { DurableObject } from 'cloudflare:workers';

// 一個帳號最多同時連線的裝置數；超過時踢掉最早的連線
const MAX_SOCKETS = 10;
// 單則訊息上限（App 端文字上限 64 KB，加密與編碼後約 90 KB）
const MAX_MESSAGE = 200_000;

export class Relay extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair('ping', 'pong'));
  }

  async fetch(request) {
    if (request.headers.get('Upgrade') !== 'websocket') return new Response(null, { status: 426 });
    const open = this.ctx.getWebSockets();
    for (const old of open.slice(0, Math.max(0, open.length - MAX_SOCKETS + 1))) {
      try {
        old.close(4000, 'replaced');
      } catch {}
    }
    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1]);
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  webSocketMessage(ws, message) {
    if (typeof message !== 'string' || message.length > MAX_MESSAGE) return;
    for (const other of this.ctx.getWebSockets()) {
      if (other === ws) continue;
      try {
        other.send(message);
      } catch {}
    }
  }

  webSocketClose(ws, code, reason) {
    try {
      ws.close(code, reason);
    } catch {}
  }

  webSocketError(ws) {
    try {
      ws.close(1011, 'error');
    } catch {}
  }
}
