import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';

const config = JSON.parse(await readFile(new URL('./site.config.json', import.meta.url), 'utf8'));
const origin = new URL(config.url);
if (origin.origin !== 'https://leigod.cmmuu.com' || origin.href !== origin.origin + '/' || !/^[a-f0-9]{32}$/.test(config.indexNowKey)) throw new Error('Unexpected submission target or ownership key.');
const expected = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: fileURLToPath(new URL('..', import.meta.url)), encoding: 'utf8' }).trim();
const localHtml = await readFile(new URL('./dist/index.html', import.meta.url), 'utf8');
if (!localHtml.includes(`<meta name="site-revision" content="${expected}">`)) throw new Error('Build this revision before submission.');
const keyUrl = new URL(`/${config.indexNowKey}.txt`, origin).href;
async function get(url) {
  const response = await fetch(url, { redirect: 'error', signal: AbortSignal.timeout(15000), headers: { 'User-Agent': 'LeigodGuard-WebsiteCheck/1.0' } });
  if (response.status !== 200) throw new Error(`Website check returned HTTP ${response.status}`);
  return response.text();
}
// Do not notify crawlers about a revision that has not reached the production site.
let deployed = false;
for (let attempt = 0; attempt < 16; attempt++) {
  try {
    const html = await get(origin.href);
    if (html === localHtml && (await get(keyUrl)).trim() === config.indexNowKey) {
      deployed = true;
      break;
    }
  } catch {
    // A bounded wait tolerates the first deploy and brief CDN/network errors.
  }
  if (attempt < 15) await delay(15000);
}
if (!deployed) throw new Error('Production did not match this build in time; no URLs were submitted.');
if (process.argv.includes('--verify-only')) {
  console.log('Production matches the built revision; ownership file is readable. No submission sent.');
  process.exit(0);
}
const response = await fetch('https://api.indexnow.org/indexnow', {
  method: 'POST', redirect: 'error', signal: AbortSignal.timeout(20000),
  headers: { 'Content-Type': 'application/json; charset=utf-8', 'User-Agent': 'LeigodGuard-WebsiteCheck/1.0' },
  body: JSON.stringify({ host: origin.hostname, key: config.indexNowKey, keyLocation: keyUrl, urlList: [origin.href] })
});
if (![200, 202].includes(response.status)) throw new Error(`IndexNow rejected the submission: HTTP ${response.status}.`);
console.log(response.status === 200
  ? 'IndexNow received the homepage URL (HTTP 200). Receipt does not confirm crawling or indexing.'
  : 'IndexNow received the homepage URL (HTTP 202); ownership validation is pending. Indexing is not confirmed.');
