// Anonymous release discovery only. Never forward visitor headers or credentials.
const REPOS = { gitee: 'https://gitee.com/cmmuu/leigod-guard', github: 'https://github.com/CMMUU/leigod-guard' };
const APIS = { gitee: 'https://gitee.com/api/v5/repos/cmmuu/leigod-guard/releases', github: 'https://api.github.com/repos/CMMUU/leigod-guard/releases' };
const MODES = ['auto', 'gitee', 'github'];
const TAG = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const HASH = /^[a-f0-9]{64}$/;
const HEADERS = { 'X-Robots-Tag': 'noindex', 'X-Content-Type-Options': 'nosniff', 'Cache-Control': 'no-store', 'Referrer-Policy': 'no-referrer' };

export function compareTags(a, b) {
  if (!TAG.test(a) || !TAG.test(b)) throw new Error('Invalid release version');
  const aa = a.slice(1).split('.').map(BigInt), bb = b.slice(1).split('.').map(BigInt);
  for (let i = 0; i < 3; i++) if (aa[i] !== bb[i]) return aa[i] > bb[i] ? 1 : -1;
  return 0;
}

function trustedRedirect(value, source) {
  const u = new URL(value);
  if (u.protocol !== 'https:' || u.username || u.password || u.port || u.hash) return false;
  if (source === 'gitee') return (u.hostname === 'foruda.gitee.com' && u.pathname.startsWith('/attach_file/')) ||
    (u.origin === 'https://gitee.com' && u.pathname.startsWith('/api/v5/repos/cmmuu/leigod-guard/releases/'));
  return ['release-assets.githubusercontent.com', 'objects.githubusercontent.com', 'github-releases.githubusercontent.com'].includes(u.hostname) ||
    (u.origin === 'https://github.com' && u.pathname.startsWith('/CMMUU/leigod-guard/releases/download/'));
}

async function readLimited(url, source, signal, fetcher, limit = 1_048_576) {
  for (let hops = 0; hops < 4; hops++) {
    const response = await fetcher(url, { signal, redirect: 'manual', headers: { 'User-Agent': 'LeigodGuard-Website/1.0', Accept: '*/*' } });
    if ([301, 302, 303, 307, 308].includes(response.status)) {
      const location = response.headers.get('location');
      await response.body?.cancel();
      if (!location) throw new Error('Missing redirect');
      url = new URL(location, url).href;
      if (!trustedRedirect(url, source)) throw new Error('Unexpected release redirect');
      continue;
    }
    if (!response.ok) { await response.body?.cancel(); throw new Error(`Release HTTP ${response.status}`); }
    const reader = response.body.getReader();
    const chunks = []; let size = 0;
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        size += value.byteLength;
        if (size > limit) throw new Error('Release response too large');
        chunks.push(value);
      }
    } finally { await reader.cancel(); }
    const bytes = new Uint8Array(size); let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  }
  throw new Error('Too many release redirects');
}

export async function loadRelease(source, version, signal, fetcher = fetch) {
  const get = async url => JSON.parse(await readLimited(url, source, signal, fetcher));
  let release;
  if (version || source === 'github') {
    release = await get(`${APIS[source]}/${version ? `tags/${version}` : 'latest'}`);
  } else {
    const releases = await get(`${APIS.gitee}?per_page=100`);
    if (!Array.isArray(releases)) throw new Error('Invalid release list');
    release = releases.filter(r => r.prerelease === false && !r.draft && TAG.test(r.tag_name))
      .sort((a, b) => compareTags(b.tag_name, a.tag_name))[0];
  }
  if (!release || release.prerelease !== false || release.draft || !TAG.test(release.tag_name) ||
      (version && release.tag_name !== version) || !Number.isSafeInteger(release.id) || release.id <= 0) throw new Error('No stable release');
  const tag = release.tag_name;
  const assets = source === 'github' ? release.assets : await get(`${APIS.gitee}/${release.id}/attach_files?per_page=100`);
  if (!Array.isArray(assets)) throw new Error('Invalid assets');
  const names = { installer: `leigod-guard-${tag}-windows-x64-setup.exe`, portable: `leigod-guard-${tag}-windows-x64.zip`, checksums: 'SHA256SUMS.txt' };
  const files = {};
  for (const [kind, name] of Object.entries(names)) {
    const matches = assets.filter(a => a.name === name);
    const asset = matches[0];
    if (matches.length !== 1 || !Number.isSafeInteger(asset.id) || asset.id <= 0 ||
        !Number.isSafeInteger(asset.size) || asset.size <= 0 || asset.size > (kind === 'checksums' ? 32768 : 536870912) ||
        asset.browser_download_url !== `${REPOS[source]}/releases/download/${tag}/${name}` ||
        (source === 'github' && asset.state !== 'uploaded')) throw new Error('Incomplete or invalid release assets');
    if (source === 'gitee' && (!Array.isArray(release.assets) || release.assets.filter(a => a.name === name && a.browser_download_url === asset.browser_download_url).length !== 1)) throw new Error('Gitee attachment mismatch');
    files[kind] = { name, size: asset.size, url: source === 'gitee' ? `${APIS.gitee}/${release.id}/attach_files/${asset.id}/download` : asset.browser_download_url, digest: asset.digest };
  }
  const manifest = await readLimited(files.checksums.url, source, signal, fetcher, 32768);
  if (new TextEncoder().encode(manifest).length !== files.checksums.size) throw new Error('Checksum size mismatch');
  const hashes = new Map();
  for (const line of manifest.replace(/^\uFEFF/, '').split(/\r?\n/).filter(Boolean)) {
    const match = /^([a-fA-F0-9]{64}) [ *]([^/\\\0]+)$/.exec(line);
    if (!match || hashes.has(match[2])) throw new Error('Invalid checksum manifest');
    hashes.set(match[2], match[1].toLowerCase());
  }
  for (const kind of ['installer', 'portable']) {
    const file = files[kind]; file.sha256 = hashes.get(file.name);
    if (!HASH.test(file.sha256 || '') || (file.digest && file.digest.toLowerCase() !== `sha256:${file.sha256}`)) throw new Error('Checksum mismatch');
    delete file.digest;
  }
  return { source, version: tag, files, checkedAt: new Date().toISOString() };
}

export function selectRelease(results, mode = 'auto', pinned) {
  const releases = results.filter(r => r && (mode === 'auto' || r.source === mode));
  const valid = releases.filter(r => !pinned || (r.version === pinned.version && r.files[pinned.edition].size === pinned.size && r.files[pinned.edition].sha256 === pinned.sha256));
  valid.sort((a, b) => compareTags(b.version, a.version) || (a.source === b.source ? 0 : a.source === 'gitee' ? -1 : 1));
  if (!valid.length) throw new Error('No matching complete release');
  const primary = valid[0];
  const mirrors = valid.filter(r => r.version === primary.version && ['installer', 'portable'].every(k => r.files[k].size === primary.files[k].size && r.files[k].sha256 === primary.files[k].sha256));
  // Conflicting same-version assets must never be presented as interchangeable.
  if (valid.some(r => r.version === primary.version && !mirrors.includes(r))) throw new Error('Release mirrors disagree');
  return { primary, mirrors };
}

async function discover(request, mode, version, context, cache, fetcher) {
  const sources = mode === 'auto' ? ['gitee', 'github'] : [mode];
  const signal = AbortSignal.timeout(8000);
  const results = await Promise.allSettled(sources.map(async source => {
    const key = new Request(`${new URL(request.url).origin}/__release-cache/v1/${source}/${version || 'latest'}`);
    const cached = await cache?.match(key);
    if (cached) return cached.json();
    const release = await loadRelease(source, version, signal, fetcher);
    if (cache) context.waitUntil(cache.put(key, Response.json(release, { headers: { 'Cache-Control': 'public, max-age=300' } })).catch(() => {}));
    return release;
  }));
  return { releases: results.filter(r => r.status === 'fulfilled').map(r => r.value), partial: results.some(r => r.status === 'rejected') };
}

function downloadPath(edition, release, source = release.source) {
  const f = release.files[edition];
  return `/download/${edition}?${new URLSearchParams({ source, version: release.version, size: String(f.size), sha256: f.sha256 })}`;
}

export async function handleDownloadRequest(request, context, cache, fetcher = fetch) {
  const url = new URL(request.url), api = url.pathname === '/api/downloads';
  const edition = url.pathname.split('/')[2];
  const mode = url.searchParams.get('source') || 'auto';
  if (!['GET', 'HEAD'].includes(request.method)) return new Response('Method not allowed', { status: 405, headers: { ...HEADERS, Allow: 'GET, HEAD' } });
  const version = url.searchParams.get('version');
  const sha256 = url.searchParams.get('sha256');
  const size = Number(url.searchParams.get('size'));
  const pinned = version || sha256 || url.searchParams.has('size');
  if (!MODES.includes(mode) || (!api && !['installer', 'portable'].includes(edition)) ||
      (pinned && (api || !TAG.test(version || '') || !HASH.test(sha256 || '') || !Number.isSafeInteger(size) || size <= 0))) {
    return new Response('Invalid download request', { status: 400, headers: HEADERS });
  }
  try {
    const { releases, partial } = await discover(request, mode, version, context, cache, fetcher);
    const { primary, mirrors } = selectRelease(releases, mode, pinned ? { version, sha256, size, edition } : undefined);
    if (!api) return new Response(null, { status: 302, headers: { ...HEADERS, Location: primary.files[edition].url } });
    const downloads = Object.fromEntries(['installer', 'portable'].map(kind => [kind, {
      url: downloadPath(kind, primary, mode), size: primary.files[kind].size, sha256: primary.files[kind].sha256,
      sources: Object.fromEntries(mirrors.map(r => [r.source, downloadPath(kind, r)]))
    }]));
    return new Response(request.method === 'HEAD' ? null : JSON.stringify({ version: primary.version, source: primary.source, partial, checkedAt: primary.checkedAt, downloads }), { headers: { ...HEADERS, 'Content-Type': 'application/json; charset=utf-8' } });
  } catch {
    if (api) return Response.json({ error: '暂未查到可用的完整正式版，请稍后重试或前往发布页。' }, { status: 503, headers: HEADERS });
    const text = '<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>下载暂不可用 · 雷神守护</title><h1>暂时无法开始下载</h1><p>下载源暂不可用或文件尚未同步完成，未替换为其他版本。请返回官网重试，或在发布页手动选择。</p><p><a href="/#download">返回官网下载</a> · <a href="https://gitee.com/cmmuu/leigod-guard/releases">Gitee 发布页</a> · <a href="https://github.com/CMMUU/leigod-guard/releases/latest">GitHub 发布页</a></p></html>';
    return new Response(request.method === 'HEAD' ? null : text, { status: 503, headers: { ...HEADERS, 'Content-Type': 'text/html; charset=utf-8', 'Content-Security-Policy': "default-src 'none'; base-uri 'none'; frame-ancestors 'none'" } });
  }
}
