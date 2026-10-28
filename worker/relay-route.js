// 中繼的小工具（不依賴 Cloudflare 執行環境，方便測試）。

const DEVICE = /^[0-9a-f]{16}$/;
// skip 最多看這麼多個（一個帳號的連線上限是 10）
const MAX_SKIP = 32;

/** App 連線時帶的裝置 ID（?d=），格式不對就當作沒有。 */
export function deviceParam(url) {
  const d = new URL(url).searchParams.get('d') ?? '';
  return DEVICE.test(d) ? d : null;
}

/**
 * 訊息裡「已經在區域網路收到、不用再轉」的裝置 ID。
 * 只讀這個欄位；密文本身解不開，也不需要解。舊版 App 的訊息沒有這個欄位。
 */
export function skipSet(message) {
  if (!message.includes('"skip"')) return null;
  try {
    const skip = JSON.parse(message).skip;
    if (!Array.isArray(skip)) return null;
    return new Set(skip.slice(0, MAX_SKIP).filter((d) => typeof d === 'string' && DEVICE.test(d)));
  } catch {
    return null;
  }
}
