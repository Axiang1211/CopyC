// 即時同步中繼的 Google id_token 驗證：簽章、受眾、發行者、期限。
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { googleKeys, resetKeyCache, verifyIdToken } from '../worker/google-jwt.js';

const b64url = (buf) => Buffer.from(buf).toString('base64url');
const AUD = 'client-123.apps.googleusercontent.com';

async function keypair(kid) {
  const { privateKey, publicKey } = await crypto.subtle.generateKey(
    { name: 'RSASSA-PKCS1-v1_5', modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: 'SHA-256' },
    true,
    ['sign', 'verify'],
  );
  const jwk = { ...(await crypto.subtle.exportKey('jwk', publicKey)), kid, alg: 'RS256', use: 'sig' };
  return { privateKey, jwk };
}

async function sign(privateKey, kid, claims, alg = 'RS256') {
  const head = b64url(JSON.stringify({ alg, kid, typ: 'JWT' }));
  const body = b64url(JSON.stringify(claims));
  const sig = await crypto.subtle.sign('RSASSA-PKCS1-v1_5', privateKey, new TextEncoder().encode(`${head}.${body}`));
  return `${head}.${body}.${b64url(sig)}`;
}

const now = Date.UTC(2026, 9, 26, 12);
const base = { iss: 'https://accounts.google.com', aud: AUD, sub: '1098765', iat: now / 1000 - 10, exp: now / 1000 + 3600 };

test('valid Google id_token returns sub', async () => {
  const { privateKey, jwk } = await keypair('k1');
  const token = await sign(privateKey, 'k1', base);
  assert.equal(await verifyIdToken(token, { audience: AUD, keys: [jwk], now }), '1098765');
  // 也接受沒有 https:// 的發行者
  const t2 = await sign(privateKey, 'k1', { ...base, iss: 'accounts.google.com' });
  assert.equal(await verifyIdToken(t2, { audience: AUD, keys: [jwk], now }), '1098765');
});

test('rejects wrong audience, issuer, expiry, key and tampering', async () => {
  const { privateKey, jwk } = await keypair('k1');
  const other = await keypair('k2');
  const opts = { audience: AUD, keys: [jwk], now };
  assert.equal(await verifyIdToken(await sign(privateKey, 'k1', { ...base, aud: 'someone-else' }), opts), null);
  assert.equal(await verifyIdToken(await sign(privateKey, 'k1', { ...base, iss: 'https://evil.example' }), opts), null);
  assert.equal(await verifyIdToken(await sign(privateKey, 'k1', { ...base, exp: now / 1000 - 3600 }), opts), null);
  // 用別把金鑰簽、卻宣稱是 k1
  assert.equal(await verifyIdToken(await sign(other.privateKey, 'k1', base), opts), null);
  // 不認識的 kid
  assert.equal(await verifyIdToken(await sign(other.privateKey, 'k2', base), opts), null);
  // 改掉 payload（換成別人的 sub）
  const good = await sign(privateKey, 'k1', base);
  const [h, , s] = good.split('.');
  const forged = `${h}.${b64url(JSON.stringify({ ...base, sub: 'attacker' }))}.${s}`;
  assert.equal(await verifyIdToken(forged, opts), null);
  // alg 不是 RS256
  assert.equal(await verifyIdToken(await sign(privateKey, 'k1', base, 'none'), opts), null);
  for (const junk of ['', 'a.b', 'a.b.c', null, undefined]) {
    assert.equal(await verifyIdToken(junk, opts), null);
  }
  // 沒有設定受眾時一律拒絕
  assert.equal(await verifyIdToken(good, { ...opts, audience: '' }), null);
});

test('Google certs are cached per Cache-Control', async () => {
  resetKeyCache();
  let calls = 0;
  const fake = async () => {
    calls += 1;
    return new Response(JSON.stringify({ keys: [{ kid: 'x' }] }), { headers: { 'Cache-Control': 'public, max-age=100' } });
  };
  assert.deepEqual(await googleKeys(fake, 0), [{ kid: 'x' }]);
  await googleKeys(fake, 50_000);
  assert.equal(calls, 1);
  await googleKeys(fake, 101_000);
  assert.equal(calls, 2);
  resetKeyCache();
});
