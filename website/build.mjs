import { createHash } from 'node:crypto';
import { readFile, writeFile, mkdir, rm, lstat, realpath } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const source = path.dirname(fileURLToPath(import.meta.url));
const repo = path.dirname(source);
const output = path.resolve(source, 'dist');
const config = JSON.parse(await readFile(path.join(source, 'site.config.json'), 'utf8'));
const site = new URL(config.url);
if (site.protocol !== 'https:' || site.username || site.password || site.search || site.hash || site.pathname !== '/') {
  throw new Error('The production site URL must be a plain HTTPS origin.');
}
if (!/^[a-f0-9]{32}$/.test(config.indexNowKey)) throw new Error('Invalid IndexNow ownership key.');
const revision = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: repo, encoding: 'utf8' }).trim();
const modified = execFileSync('git', ['log', '-1', '--format=%cI', '--', 'website', 'assets/app-icon.png', 'assets/ui-home.png', 'Cargo.toml', 'README.md'], { cwd: repo, encoding: 'utf8' }).trim();
if (!/^[a-f0-9]{40}$/.test(revision) || !Number.isFinite(Date.parse(modified))) throw new Error('Missing source revision/date.');
const lastModified = new Date(modified).toISOString();
const faq = JSON.parse(await readFile(path.join(source, 'faq.json'), 'utf8'));
if (!Array.isArray(faq) || !faq.length || faq.some(item => typeof item.question !== 'string' || typeof item.answer !== 'string' || !item.question.trim() || !item.answer.trim())) throw new Error('Invalid FAQ content.');
const escapeHtml = value => value.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;');
const faqHtml = faq.map((item, i) => `<article class="faq-row"><span class="faq-number" aria-hidden="true">${String(i + 1).padStart(2, '0')}</span><div><h3>${escapeHtml(item.question)}</h3><p>${escapeHtml(item.answer)}</p></div></article>`).join('\n        ');
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
  const inputBytes = await readFile(input);
  const bytes = ['css', 'js'].includes(extension) ? Buffer.from(inputBytes.toString('utf8').replaceAll('\r\n', '\n')) : inputBytes;
  const hash = createHash('sha256').update(bytes).digest('hex').slice(0, 12);
  const relative = `assets/${name}.${hash}.${extension}`;
  await writeFile(path.join(output, relative), bytes);
  return relative;
}
const icon = await asset(path.join(repo, 'assets', 'app-icon.png'), 'app-icon', 'png');
const screenshot = await asset(path.join(repo, 'assets', 'ui-home.png'), 'ui-home', 'png');
const stylesheet = await asset(path.join(source, 'styles.css'), 'styles', 'css');
const downloadScript = await asset(path.join(source, 'downloads.js'), 'downloads', 'js');
const cargo = await readFile(path.join(repo, 'Cargo.toml'), 'utf8');
const version = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) throw new Error('Could not read the application version.');
const application = {
  '@type': 'SoftwareApplication',
  '@id': site.origin + '/#software',
  name: config.name,
  alternateName: ['雷神守护', 'Leigod Guard', 'leigod-guard'],
  url: site.origin + '/',
  description: '配合雷神加速器使用的独立开源 Windows 计时守护工具，支持按游戏进程暂停计时与查询剩余时长。',
  applicationCategory: 'UtilitiesApplication',
  operatingSystem: 'Windows 10 / 11 x64',
  softwareVersion: version,
  downloadUrl: site.origin + '/download/installer',
  license: 'https://github.com/CMMUU/leigod-guard/blob/main/LICENSE',
  screenshot: site.origin + '/' + screenshot,
  image: site.origin + '/' + icon,
  sameAs: ['https://github.com/CMMUU/leigod-guard', 'https://gitee.com/cmmuu/leigod-guard'],
  featureList: ['按游戏进程自动暂停账户计时', '本次启动无游戏检查与准备游戏延后', '查询账户剩余时长', '安装版与绿色版', 'Gitee 与 GitHub 应用内更新'],
  isAccessibleForFree: true
};
const structuredData = JSON.stringify({
  '@context': 'https://schema.org',
  '@graph': [
    { '@type': 'WebSite', '@id': site.origin + '/#website', url: site.origin + '/', name: config.name, inLanguage: 'zh-CN' },
    { '@type': 'WebPage', '@id': site.origin + '/#webpage', url: site.origin + '/', name: '雷神守护 · 雷神加速器自动暂停计时工具', inLanguage: 'zh-CN', dateModified: lastModified, isPartOf: { '@id': site.origin + '/#website' }, about: { '@id': application['@id'] } },
    application,
    { '@type': 'FAQPage', '@id': site.origin + '/#faq', url: site.origin + '/#faq', isPartOf: { '@id': site.origin + '/#webpage' }, mainEntity: faq.map(item => ({ '@type': 'Question', name: item.question, acceptedAnswer: { '@type': 'Answer', text: item.answer } })) }
  ]
}).replaceAll('<', '\\u003c');
const replacements = { SITE_URL: site.origin, SITE_REVISION: revision, APP_ICON: icon, APP_SCREENSHOT: screenshot, STYLESHEET: stylesheet, DOWNLOAD_SCRIPT: downloadScript, STRUCTURED_DATA: structuredData, FAQ_HTML: faqHtml, APP_VERSION: version, LAST_MODIFIED: lastModified, FAQ_MARKDOWN: faq.map(item => `### ${item.question}\n\n${item.answer}`).join('\n\n') };
function render(template) {
  const result = template.replaceAll('\r\n', '\n').replace(/\{\{([A-Z_]+)\}\}/g, (_, key) => {
    if (!(key in replacements)) throw new Error(`Unknown template field: ${key}`);
    return replacements[key];
  });
  if (result.includes('{{')) throw new Error('Unresolved template field.');
  return result;
}
const template = await readFile(path.join(source, 'index.html'), 'utf8');
const html = render(template);
const jsonHash = createHash('sha256').update(structuredData).digest('base64');
await writeFile(path.join(output, 'index.html'), html);
await writeFile(path.join(output, '404.html'), render(await readFile(path.join(source, '404.html'), 'utf8')));
const overview = render(await readFile(path.join(source, 'overview.md'), 'utf8'));
await writeFile(path.join(output, 'index.html.md'), overview);
const readme = (await readFile(path.join(repo, 'README.md'), 'utf8')).replaceAll('\r\n', '\n');
const sections = ['账户剩余时长与自动刷新', '自动暂停规则', '如何确认暂停生效'];
const rules = sections.map(title => {
  const marker = `## ${title}\n`;
  const start = readme.indexOf(marker);
  if (start < 0) throw new Error(`Missing README section: ${title}`);
  const end = readme.indexOf('\n## ', start + marker.length);
  return readme.slice(start, end < 0 ? undefined : end).trim();
}).join('\n\n');
await writeFile(path.join(output, 'llms-full.txt'), `${overview}\n\n---\n\n# README 详细规则摘录\n\n来源：https://github.com/CMMUU/leigod-guard/blob/${revision}/README.md\n\n${rules}\n`);
await writeFile(path.join(output, 'llms.txt'), `# 雷神守护 Leigod Guard\n\n> 配合雷神加速器使用的独立开源 Windows 计时守护工具。请求暂停账户计时，不停止客户端加速。\n\n目前仅提供 Windows 10 / 11 x64 版本。断电后本地程序无法工作，重启检查需要有效策略、名单、登录与网络。网站时长为演示值，不查询访客账户。此文件是公开资料索引，不代表任何搜索引擎或 AI 已收录本项目。\n\n## 项目资料\n\n- [项目概览与常见问题](${site.origin}/index.html.md): 首页信息的 Markdown 版本，包含下载、适用范围和能力边界。\n- [完整资料与暂停规则](${site.origin}/llms-full.txt): 合并项目概览与 README 的刷新、暂停和核对规则。\n- [项目首页](${site.origin}/): 面向用户的单页下载与介绍入口。\n\n## 下载与源码\n\n- [Gitee 发布页](https://gitee.com/cmmuu/leigod-guard/releases): 国内下载，选择安装版或绿色版。\n- [GitHub 最新正式版](https://github.com/CMMUU/leigod-guard/releases/latest): 主发布源与备用下载。\n- [GitHub 源码](https://github.com/CMMUU/leigod-guard): 源码、版本历史、问题反馈。\n\n## Optional\n\n- [隐私说明](https://gitee.com/cmmuu/leigod-guard/blob/main/docs/PRIVACY.md): 本地数据与联网范围。\n- [MIT 许可证](https://gitee.com/cmmuu/leigod-guard/blob/main/LICENSE): 开源许可。\n`);
await writeFile(path.join(output, config.indexNowKey + '.txt'), config.indexNowKey + '\n');
await writeFile(path.join(output, '_headers'), `/*
  X-Content-Type-Options: nosniff
  Referrer-Policy: strict-origin-when-cross-origin
  X-Frame-Options: DENY
  Permissions-Policy: camera=(), microphone=(), geolocation=()
  Content-Security-Policy: default-src 'none'; style-src 'self'; img-src 'self'; script-src 'self' 'sha256-${jsonHash}'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'
/assets/*
  Cache-Control: public, max-age=31536000, immutable
https://leigod-guard.pages.dev/*
  X-Robots-Tag: noindex
  Link: <${site.origin}/>; rel="canonical"
/index.html.md
  Content-Type: text/markdown; charset=utf-8
  Link: <${site.origin}/>; rel="canonical", <${site.origin}/llms.txt>; rel="describedby"
/llms.txt
  Content-Type: text/plain; charset=utf-8
/llms-full.txt
  Content-Type: text/plain; charset=utf-8
/${config.indexNowKey}.txt
  X-Robots-Tag: noindex
`);
const searchBots = ['Googlebot', 'Bingbot', 'Baiduspider', 'OAI-SearchBot', 'ChatGPT-User', 'PerplexityBot', 'Perplexity-User', 'Claude-SearchBot', 'Claude-User'];
await writeFile(path.join(output, '_routes.json'), JSON.stringify({ version: 1, include: ['/api/downloads', '/download/*'], exclude: [] }));
const trainingBots = ['GPTBot', 'ClaudeBot', 'CCBot', 'Google-Extended', 'Applebot-Extended', 'Bytespider', 'meta-externalagent'];
await writeFile(path.join(output, 'robots.txt'), `# Search and answer retrieval are welcome. Training preference is unchanged.\nUser-agent: *\nAllow: /\nContent-Signal: search=yes,ai-input=yes,ai-train=no,use=reference\n\n${searchBots.map(bot => `User-agent: ${bot}\nAllow: /\nContent-Signal: search=yes,ai-input=yes,ai-train=no,use=reference`).join('\n\n')}\n\n${trainingBots.map(bot => `User-agent: ${bot}\nDisallow: /`).join('\n\n')}\n\nSitemap: ${site.origin}/sitemap.xml\n`);
await writeFile(path.join(output, 'sitemap.xml'), `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>${site.origin}/</loc><lastmod>${lastModified}</lastmod></url></urlset>\n`);
console.log(`Built ${config.name} for ${site.origin}: HTML, Markdown, AI index, real 404, canonical headers, metadata and IndexNow ownership file.`);
