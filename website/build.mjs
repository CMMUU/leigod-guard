import { createHash } from 'node:crypto';
import { readFile, writeFile, mkdir, rm, lstat, realpath } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const source = path.dirname(fileURLToPath(import.meta.url));
const repo = path.dirname(source);
const output = path.resolve(source, 'dist');
const config = JSON.parse(await readFile(path.join(source, 'site.config.json'), 'utf8'));
const site = new URL(config.url);
if (site.protocol !== 'https:' || site.username || site.password || site.search || site.hash || site.pathname !== '/') {
  throw new Error('The production site URL must be a plain HTTPS origin.');
}
// The build can only replace its own non-linked output directory.
if (path.dirname(output) !== await realpath(source) || path.basename(output) !== 'dist') {
  throw new Error('Unexpected build output directory.');
}
const existing = await lstat(output).catch(error => {
  if (error.code === 'ENOENT') return null;
  throw error;
});
if (existing && (existing.isSymbolicLink() || !existing.isDirectory())) throw new Error('Build output must be a normal directory.');
await rm(output, { recursive: true, force: true });
await mkdir(path.join(output, 'assets'), { recursive: true });

async function asset(input, name, extension) {
  const bytes = await readFile(input);
  const hash = createHash('sha256').update(bytes).digest('hex').slice(0, 12);
  const relative = `assets/${name}.${hash}.${extension}`;
  await writeFile(path.join(output, relative), bytes);
  return relative;
}
const icon = await asset(path.join(repo, 'assets', 'app-icon.png'), 'app-icon', 'png');
const screenshot = await asset(path.join(repo, 'assets', 'ui-home.png'), 'ui-home', 'png');
const stylesheet = await asset(path.join(source, 'styles.css'), 'styles', 'css');
const cargo = await readFile(path.join(repo, 'Cargo.toml'), 'utf8');
const version = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) throw new Error('Could not read the application version.');
const structuredData = JSON.stringify({
  '@context': 'https://schema.org',
  '@type': 'SoftwareApplication',
  name: config.name,
  url: site.origin + '/',
  description: '配合雷神加速器使用的独立开源 Windows 计时守护工具，支持按游戏进程暂停计时与查询剩余时长。',
  applicationCategory: 'UtilitiesApplication',
  operatingSystem: 'Windows 10 / 11 x64',
  softwareVersion: version,
  downloadUrl: 'https://gitee.com/cmmuu/leigod-guard/releases',
  license: 'https://github.com/CMMUU/leigod-guard/blob/main/LICENSE',
  screenshot: site.origin + '/' + screenshot,
  isAccessibleForFree: true
}).replaceAll('<', '\\u003c');
const replacements = { SITE_URL: site.origin, APP_ICON: icon, APP_SCREENSHOT: screenshot, STYLESHEET: stylesheet, STRUCTURED_DATA: structuredData };
const template = await readFile(path.join(source, 'index.html'), 'utf8');
const html = template.replace(/\{\{([A-Z_]+)\}\}/g, (_, key) => {
  if (!(key in replacements)) throw new Error(`Unknown template field: ${key}`);
  return replacements[key];
});
if (html.includes('{{')) throw new Error('Unresolved template field.');
const jsonHash = createHash('sha256').update(structuredData).digest('base64');
await writeFile(path.join(output, 'index.html'), html);
await writeFile(path.join(output, '_headers'), `/*
  X-Content-Type-Options: nosniff
  Referrer-Policy: strict-origin-when-cross-origin
  X-Frame-Options: DENY
  Permissions-Policy: camera=(), microphone=(), geolocation=()
  Content-Security-Policy: default-src 'none'; style-src 'self'; img-src 'self'; script-src 'sha256-${jsonHash}'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'
/assets/*
  Cache-Control: public, max-age=31536000, immutable
`);
await writeFile(path.join(output, 'robots.txt'), `User-agent: *\nAllow: /\nSitemap: ${site.origin}/sitemap.xml\n`);
await writeFile(path.join(output, 'sitemap.xml'), `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>${site.origin}/</loc></url></urlset>\n`);
console.log(`Built ${config.name} for ${site.origin}: index, 3 content-hashed assets, headers, robots and sitemap.`);
