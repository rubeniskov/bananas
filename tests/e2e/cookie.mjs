// Mint a bananas-session cookie locally — same HMAC-SHA256 over
// `<username>|<expiry_unix>` URL-safe base64-no-pad as
// crates/server-common/src/session.rs::Session::sign. Sidesteps PAM
// for tests; we just want a valid cookie that webadmin's auth
// middleware accepts without going through `/api/login`.
import crypto from 'node:crypto';
import fs from 'node:fs/promises';

export async function mintCookie(keyPath, username = 'root', ttlSecs = 3600) {
  const key = await fs.readFile(keyPath);
  if (key.length < 32) {
    throw new Error(`session key at ${keyPath} too short (${key.length}<32)`);
  }
  const expiry = Math.floor(Date.now() / 1000) + ttlSecs;
  const payload = `${username}|${expiry}`;
  const payloadB64 = Buffer.from(payload).toString('base64url');
  const tagB64 = crypto.createHmac('sha256', key).update(payloadB64).digest('base64url');
  return `${payloadB64}.${tagB64}`;
}
