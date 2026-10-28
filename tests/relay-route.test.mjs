import assert from 'node:assert/strict';
import { test } from 'node:test';
import { deviceParam, skipSet } from '../worker/relay-route.js';

test('device id comes from ?d= and must be 16 hex characters', () => {
  assert.equal(deviceParam('https://x/sync/ws?d=0123456789abcdef'), '0123456789abcdef');
  assert.equal(deviceParam('https://x/sync/ws'), null);
  assert.equal(deviceParam('https://x/sync/ws?d=XYZ'), null);
  assert.equal(deviceParam('https://x/sync/ws?d=0123456789ABCDEF'), null);
});

test('skip list is read from the message without touching the ciphertext', () => {
  const msg = JSON.stringify({ v: 1, data: 'Q1BDMQ==', skip: ['0123456789abcdef', 'bad', 7] });
  assert.deepEqual([...skipSet(msg)], ['0123456789abcdef']);
  // 舊版 App 的訊息沒有 skip：全部轉送
  assert.equal(skipSet(JSON.stringify({ v: 1, data: 'Q1BDMQ==' })), null);
  assert.equal(skipSet('"skip" but not json'), null);
  assert.equal(skipSet(JSON.stringify({ v: 1, data: 'x', skip: 'nope' })), null);
});

test('skip list is capped', () => {
  const ids = Array.from({ length: 100 }, (_, i) => i.toString(16).padStart(16, '0'));
  assert.equal(skipSet(JSON.stringify({ v: 1, data: 'x', skip: ids })).size, 32);
});
