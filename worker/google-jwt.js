// 驗證 Google 的 id_token（RS256 JWT）：簽章、發行者、受眾與期限。
// 通過後回傳使用者的 sub（Google 帳號的固定 ID），用來決定即時同步的房間。

const CERTS_URL = 'https://www.googleapis.com/oauth2/v3/certs';
const ISSUERS = new Set(['accounts.google.com', 'https://accounts.google.com']);
// 容許裝置時鐘些微誤差
const SKEW_S = 60;

let cache = { keys: null, until: 0 };

function b64urlBytes(s) {
  const bin = atob(s.replace(/-/g, '+').replace(/_/g, '/') + '==='.slice((s.length + 3) % 4));
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}

function b64urlJson(s) {
  return JSON.parse(new TextDecoder().decode(b64urlBytes(s)));
}

/** 取得 Google 的公開金鑰，依 Cache-Control 快取。 */
export async function googleKeys(fetchImpl = fetch, now = Date.now()) {
  if (cache.keys && now < cache.until) return cache.keys;
  const res = await fetchImpl(CERTS_URL);
  if (!res.ok) throw new Error(`certs ${res.status}`);
  const { keys } = await res.json();
  const maxAge = Number(/max-age=(\d+)/.exec(res.headers.get('Cache-Control') ?? '')?.[1] ?? 3600);
  cache = { keys, until: now + Math.min(maxAge, 86400) * 1000 };
  return keys;
}

export function resetKeyCache() {
  cache = { keys: null, until: 0 };
}

/**
 * 驗證 token，成功回傳 sub，失敗回傳 null。
 * @param {string} token
 * @param {{ audience: string, keys: object[], now?: number }} opts
 */
export async function verifyIdToken(token, { audience, keys, now = Date.now() }) {
  const parts = typeof token === 'string' ? token.split('.') : [];
  if (parts.length !== 3 || !audience) return null;
  let header;
  let claims;
  try {
    header = b64urlJson(parts[0]);
    claims = b64urlJson(parts[1]);
  } catch {
    return null;
  }
  if (header.alg !== 'RS256') return null;
  const jwk = keys.find((k) => k.kid === header.kid);
  if (!jwk) return null;

  const key = await crypto.subtle.importKey(
    'jwk',
    { kty: jwk.kty, n: jwk.n, e: jwk.e, alg: 'RS256', ext: true },
    { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' },
    false,
    ['verify'],
  );
  const signed = new TextEncoder().encode(`${parts[0]}.${parts[1]}`);
  let ok = false;
  try {
    ok = await crypto.subtle.verify('RSASSA-PKCS1-v1_5', key, b64urlBytes(parts[2]), signed);
  } catch {
    return null;
  }
  if (!ok) return null;

  const t = now / 1000;
  const aud = Array.isArray(claims.aud) ? claims.aud : [claims.aud];
  if (!ISSUERS.has(claims.iss) || !aud.includes(audience)) return null;
  if (typeof claims.exp !== 'number' || claims.exp + SKEW_S < t) return null;
  if (typeof claims.iat === 'number' && claims.iat - SKEW_S > t) return null;
  return typeof claims.sub === 'string' && claims.sub ? claims.sub : null;
}
