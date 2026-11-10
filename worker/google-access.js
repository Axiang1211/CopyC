// Android 版送來的是 Google access token（不是 id_token）：向 Google 的 tokeninfo 確認它屬於哪個帳號，
// 而且是發給 CopyC 自己的 Android 用戶端（別的 App 拿到的 token 不算）。

const TOKENINFO = 'https://oauth2.googleapis.com/tokeninfo';

/**
 * 用戶端 ID 清單：可以是陣列，或用逗號分隔的字串（Cloudflare 祕密只能存字串）。
 * Android 版有兩個用戶端：自己簽章的測試版，以及 Google Play 重新簽章的正式版。
 */
export const clientIds = (value) =>
  (Array.isArray(value) ? value : String(value ?? '').split(','))
    .map((s) => String(s ?? '').trim())
    .filter(Boolean);

/** 看起來像 JWT（id_token）就交給 verifyIdToken。 */
export const isJwt = (token) => /^eyJ[\w-]*\.[\w-]+\.[\w-]+$/.test(token);

/**
 * 成功回傳 sub，token 無效或不是給我們的回傳 null；Google 暫時無法連線時丟出錯誤（回 502）。
 * @param {string} token
 * @param {{ audience: string | string[], fetchImpl?: typeof fetch }} opts
 */
export async function verifyAccessToken(token, { audience, fetchImpl = fetch }) {
  const audiences = clientIds(audience);
  if (!token || !audiences.length || token.length > 4096) return null;
  const res = await fetchImpl(`${TOKENINFO}?access_token=${encodeURIComponent(token)}`);
  if (res.status >= 500) throw new Error(`tokeninfo ${res.status}`);
  if (!res.ok) return null;
  const info = await res.json();
  if (!audiences.includes(info.aud) && !audiences.includes(info.azp)) return null;
  if (!(Number(info.expires_in) > 0)) return null;
  return typeof info.sub === 'string' && info.sub ? info.sub : null;
}
