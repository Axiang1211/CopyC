import assert from 'node:assert/strict';
import { test } from 'node:test';
import { clientIds, isJwt, verifyAccessToken } from '../worker/google-access.js';

const ANDROID = 'android-123.apps.googleusercontent.com';
const reply = (status, body) => async () => new Response(JSON.stringify(body), { status });

test('tells id tokens from access tokens', () => {
  assert.ok(isJwt('eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln'));
  assert.ok(!isJwt('ya29.a0AfB_byC-example_token'));
});

test('accepts an access token issued to our Android client', async () => {
  const info = { aud: ANDROID, azp: ANDROID, sub: '1098765', expires_in: '3599', scope: 'openid email' };
  assert.equal(await verifyAccessToken('ya29.x', { audience: ANDROID, fetchImpl: reply(200, info) }), '1098765');
});

test('rejects tokens for other apps, expired or invalid tokens', async () => {
  const other = { aud: 'someone-else', azp: 'someone-else', sub: '1', expires_in: '3599' };
  assert.equal(await verifyAccessToken('ya29.x', { audience: ANDROID, fetchImpl: reply(200, other) }), null);
  const expired = { aud: ANDROID, sub: '1', expires_in: '0' };
  assert.equal(await verifyAccessToken('ya29.x', { audience: ANDROID, fetchImpl: reply(200, expired) }), null);
  assert.equal(await verifyAccessToken('ya29.x', { audience: ANDROID, fetchImpl: reply(400, { error: 'invalid_token' }) }), null);
  // 沒設定 Android 用戶端：一律拒絕
  assert.equal(await verifyAccessToken('ya29.x', { audience: undefined, fetchImpl: reply(200, other) }), null);
  await assert.rejects(verifyAccessToken('ya29.x', { audience: ANDROID, fetchImpl: reply(503, {}) }));
});

test('accepts several Android client IDs separated by commas', async () => {
  assert.deepEqual(clientIds(' a.apps , b.apps ,'), ['a.apps', 'b.apps']);
  assert.deepEqual(clientIds(undefined), []);
  const info = { aud: 'play-signed', azp: 'play-signed', sub: '42', expires_in: '100' };
  const fetchImpl = async () => new Response(JSON.stringify(info), { status: 200 });
  assert.equal(await verifyAccessToken('ya29.x', { audience: `${ANDROID}, play-signed`, fetchImpl }), '42');
});
