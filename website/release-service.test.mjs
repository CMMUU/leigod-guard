import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { compareTags, loadRelease, selectRelease, handleDownloadRequest } from './release-service.mjs';

function fixture(source, version = 'v0.12.1') {
  const repo = source === 'gitee' ? 'https://gitee.com/cmmuu/leigod-guard' : 'https://github.com/CMMUU/leigod-guard';
  const api = source === 'gitee' ? 'https://gitee.com/api/v5/repos/cmmuu/leigod-guard/releases' : 'https://api.github.com/repos/CMMUU/leigod-guard/releases';
  const names = [`leigod-guard-${version}-windows-x64-setup.exe`, `leigod-guard-${version}-windows-x64.zip`, 'SHA256SUMS.txt'];
  const checksum = `${'a'.repeat(64)}  ${names[0]}\n${'b'.repeat(64)}  ${names[1]}\n`;
  const assets = names.map((name, i) => ({ id: i + 41, name, size: i === 2 ? Buffer.byteLength(checksum) : 1024 + i, state: 'uploaded', browser_download_url: `${repo}/releases/download/${version}/${name}` }));
  const release = { id: 7, tag_name: version, prerelease: false, draft: false, assets };
  const routes = new Map([
    [`${api}/latest`, release], [`${api}/tags/${version}`, release], [`${api}?per_page=100`, [release]],
    [`${api}/7/attach_files?per_page=100`, assets],
    [source === 'gitee' ? `${api}/7/attach_files/43/download` : assets[2].browser_download_url, checksum]
  ]);
  return { release, assets, routes, checksum };
}

function transport(...fixtures) {
  const routes = new Map(fixtures.flatMap(f => [...f.routes]));
  const calls = [];
  const fetcher = async (url, options) => {
    calls.push({ url, options });
    if (!routes.has(url)) throw new Error('Source unavailable');
    const data = routes.get(url);
    return data instanceof Response ? data.clone() : typeof data === 'string' ? new Response(data) : Response.json(data);
  };
  return { fetcher, calls, routes };
}
const context = { waitUntil: promise => promise };
const request = path => new Request(`https://leigod.cmmuu.com${path}`);
const parsed = async (source, version) => loadRelease(source, version, AbortSignal.timeout(1000), transport(fixture(source, version)).fetcher);

function centerFixture(version = 'v0.19.0') {
  const upstream = fixture('github', version);
  const files = upstream.assets.map((a, i) => {
    const id = createHash('sha256').update(version + a.name).digest('hex');
    return { id, filename: a.name, version, size: a.size, platform: 'windows', architecture: 'x64',
      sha256: i === 2 ? createHash('sha256').update(upstream.checksum).digest('hex') : (i ? 'b' : 'a').repeat(64),
      source: 'manual', project: null, category: '加速器守护', status: 'published',
      downloadUrl: `https://files.cmmuu.com/d/${id}/${a.name}` };
  });
  const catalog = { files };
  return { files, catalog, routes: new Map([
    ['https://downloads.cmmuu.com/api/catalog', catalog],
    [files[2].downloadUrl, upstream.checksum]
  ]) };
}

test('default website source uses center direct URLs and fixed entries redirect without upstream credentials', async () => {
  const f = centerFixture(), network = transport(f);
  const response = await handleDownloadRequest(request('/api/downloads'), context, null, network.fetcher);
  assert.equal(response.status, 200);
  const result = await response.json();
  assert.equal(result.source, 'center'); assert.equal(result.version, 'v0.19.0');
  assert.equal(result.downloads.installer.url, f.files[0].downloadUrl);
  for (const [i, edition] of ['installer', 'portable'].entries()) {
    const r = await handleDownloadRequest(request(`/download/${edition}`), context, null, network.fetcher);
    assert.equal(r.status, 302); assert.equal(r.headers.get('location'), f.files[i].downloadUrl);
  }
  assert.ok(network.calls.every(c => new URL(c.url).hostname.endsWith('.cmmuu.com')));
  assert.ok(network.calls.every(c => !('Cookie' in c.options.headers) && !('Authorization' in c.options.headers)));
});

test('new center version updates the same fixed entry; order and unrelated projects do not matter', async () => {
  const old = centerFixture(), newer = centerFixture('v0.20.0');
  newer.catalog.files = [old.files[0], ...newer.files, ...old.files.slice(1),
    { ...newer.files[0], project: 'serylane', category: '项目发行', version: 'v9.0.0' }];
  const network = transport(old, newer);
  const r = await handleDownloadRequest(request('/download/installer'), context, null, network.fetcher);
  assert.equal(r.headers.get('location'), newer.files[0].downloadUrl);
});

test('center pins retain old version and checksum when a new version is published', async () => {
  const old = centerFixture(), newer = centerFixture('v0.20.0');
  const initial = await handleDownloadRequest(request('/api/downloads'), context, null, transport(old).fetcher);
  const pin = (await initial.json()).downloads.installer.sources.center;
  newer.catalog.files = [...newer.files, ...old.files];
  const network = transport(old, newer);
  const response = await handleDownloadRequest(request(pin), context, null, network.fetcher);
  assert.equal(response.headers.get('location'), old.files[0].downloadUrl);
  assert.equal((await handleDownloadRequest(request(pin.replace('a'.repeat(64), 'c'.repeat(64))), context, null, network.fetcher)).status, 503);
});

for (const defect of ['missing', 'duplicate', 'draft', 'wrongPlatform', 'wrongUrl', 'wrongSize', 'wrongHash', 'changedManifest', 'redirect']) {
  test(`download center rejects ${defect} and does not silently switch sources`, async () => {
    const f = centerFixture();
    if (defect === 'missing') f.files.splice(1, 1);
    if (defect === 'duplicate') f.files.push({ ...f.files[0] });
    if (defect === 'draft') f.files[1].status = 'draft';
    if (defect === 'wrongPlatform') f.files[0].architecture = 'arm64';
    if (defect === 'wrongUrl') f.files[0].downloadUrl = 'https://evil.example/install.exe';
    if (defect === 'wrongSize') f.files[0].size = 0;
    if (defect === 'wrongHash') f.files[0].sha256 = 'c'.repeat(64);
    if (defect === 'changedManifest') f.routes.set(f.files[2].downloadUrl, 'changed');
    if (defect === 'redirect') f.routes.set(f.files[2].downloadUrl, new Response(null, { status: 302, headers: { Location: 'https://evil.example/checksums' } }));
    const network = transport(f, fixture('github'), fixture('gitee'));
    const response = await handleDownloadRequest(request('/api/downloads'), context, null, network.fetcher);
    assert.equal(response.status, 503);
    assert.ok(network.calls.every(c => new URL(c.url).hostname.endsWith('.cmmuu.com')));
  });
}

test('an incomplete newly published center version is not replaced with an older latest claim', async () => {
  const f = centerFixture(); f.files.push(centerFixture('v0.20.0').files[0]);
  assert.equal((await handleDownloadRequest(request('/api/downloads'), context, null, transport(f).fetcher)).status, 503);
});

test('center metadata cache expires after one minute and HEAD redirects without a body', async () => {
  const pending = [], saved = [];
  const cache = { match: async () => undefined, put: async (_key, r) => saved.push(r.headers.get('cache-control')) };
  const ctx = { waitUntil: p => pending.push(p) };
  const r = await handleDownloadRequest(new Request('https://leigod.cmmuu.com/download/portable', { method: 'HEAD' }), ctx, cache, transport(centerFixture()).fetcher);
  await Promise.all(pending);
  assert.equal(r.status, 302); assert.equal(await r.text(), '');
  assert.deepEqual(saved, ['public, max-age=60']);
});

test('semantic versions compare numerically and reject prerelease / malformed tags', () => {
  assert.ok(compareTags('v0.12.1', 'v0.9.99') > 0);
  assert.throws(() => compareTags('v01.2.3', 'v1.2.3'));
  assert.throws(() => compareTags('v1.2.3-beta', 'v1.2.3'));
});

test('equal complete versions prefer Gitee, newer complete GitHub wins', async () => {
  const gitee = await parsed('gitee', 'v0.12.1');
  assert.equal(selectRelease([await parsed('github', 'v0.12.1'), gitee]).primary.source, 'gitee');
  assert.equal(selectRelease([gitee, await parsed('github', 'v0.13.0')]).primary.version, 'v0.13.0');
});

for (const mode of ['gitee', 'github']) test(`strict ${mode} mode makes no requests to other source`, async () => {
  const network = transport(fixture(mode));
  const response = await handleDownloadRequest(request(`/api/downloads?source=${mode}`), context, null, network.fetcher);
  assert.equal(response.status, 200);
  const result = await response.json();
  assert.equal(result.source, mode); assert.equal(result.partial, false);
  assert.deepEqual(Object.keys(result.downloads.installer.sources), [mode]);
  assert.ok(network.calls.every(c => c.url.includes(mode === 'gitee' ? 'gitee.com' : 'github.com')));
  assert.ok(network.calls.every(c => !('Cookie' in c.options.headers) && !('Authorization' in c.options.headers)));
});

test('one source offline returns partial result; both offline return an error', async () => {
  const response = await handleDownloadRequest(request('/api/downloads?source=auto'), context, null, transport(fixture('github')).fetcher);
  assert.equal(response.status, 200); assert.equal((await response.json()).partial, true);
  const failed = await handleDownloadRequest(request('/api/downloads?source=auto'), context, null, transport().fetcher);
  assert.equal(failed.status, 503);
  assert.equal(failed.headers.get('cache-control'), 'no-store');
});

test('aborted upstream requests produce bounded unavailability, never a latest claim', async () => {
  const fetcher = async (_url, options) => { assert.ok(options.signal instanceof AbortSignal); throw new DOMException('Timeout', 'TimeoutError'); };
  const response = await handleDownloadRequest(request('/api/downloads?source=auto'), context, null, fetcher);
  assert.equal(response.status, 503);
});

for (const defect of ['prerelease', 'draft', 'missingZip', 'duplicate', 'wrongUrl', 'wrongSize', 'badHash', 'digestMismatch']) {
  test(`reject ${defect} release`, async () => {
    const f = fixture('github');
    if (defect === 'prerelease') f.release.prerelease = true;
    if (defect === 'draft') f.release.draft = true;
    if (defect === 'missingZip') f.assets.splice(1, 1);
    if (defect === 'duplicate') f.assets.push(f.assets[0]);
    if (defect === 'wrongUrl') f.assets[0].browser_download_url = 'https://evil.example/install.exe';
    if (defect === 'wrongSize') f.assets[0].size = 0;
    if (defect === 'badHash') f.routes.set(f.assets[2].browser_download_url, f.checksum.replace('aaaa', 'zzzz'));
    if (defect === 'digestMismatch') f.assets[0].digest = `sha256:${'c'.repeat(64)}`;
    await assert.rejects(loadRelease('github', undefined, AbortSignal.timeout(1000), transport(f).fetcher));
  });
}

test('Gitee latest is highest stable semantic tag, not list order or draft', async () => {
  const f = fixture('gitee');
  const list = f.routes.get('https://gitee.com/api/v5/repos/cmmuu/leigod-guard/releases?per_page=100');
  list.unshift({ ...f.release, tag_name: 'v0.9.0' }, { ...f.release, tag_name: 'v9.0.0', prerelease: true });
  const result = await loadRelease('gitee', undefined, AbortSignal.timeout(1000), transport(f).fetcher);
  assert.equal(result.version, 'v0.12.1');
});

test('newest incomplete source does not pretend its older release is latest', async () => {
  const f = fixture('gitee'); f.assets.pop();
  const network = transport(f, fixture('github'));
  const response = await handleDownloadRequest(request('/api/downloads?source=auto'), context, null, network.fetcher);
  const result = await response.json();
  assert.equal(result.source, 'github'); assert.equal(result.partial, true);
});

test('untrusted checksum redirects and oversized upstream responses are rejected', async () => {
  const f = fixture('gitee');
  f.routes.set('https://gitee.com/api/v5/repos/cmmuu/leigod-guard/releases/7/attach_files/43/download', new Response(null, { status: 302, headers: { Location: 'https://evil.example/secret' } }));
  const network = transport(f);
  await assert.rejects(loadRelease('gitee', undefined, AbortSignal.timeout(1000), network.fetcher));
  assert.ok(network.calls.every(c => !c.url.includes('evil.example')));
  f.routes.set('https://gitee.com/api/v5/repos/cmmuu/leigod-guard/releases?per_page=100', ' '.repeat(1_048_577));
  await assert.rejects(loadRelease('gitee', undefined, AbortSignal.timeout(1000), transport(f).fetcher));
});

test('same version conflicting mirrors fail closed', async () => {
  const gitee = await parsed('gitee', 'v0.12.1'), github = await parsed('github', 'v0.12.1');
  github.files.installer.sha256 = 'c'.repeat(64);
  assert.throws(() => selectRelease([gitee, github]));
});

test('pinned fallback preserves version, edition, size and checksum despite a newer latest', async () => {
  const initial = await handleDownloadRequest(request('/api/downloads?source=auto'), context, null, transport(fixture('gitee'), fixture('github')).fetcher);
  const link = (await initial.json()).downloads.installer.url;
  const old = fixture('github');
  const newer = fixture('github', 'v0.13.0');
  const network = transport(old, newer); // Gitee becomes unavailable after the page loaded.
  const response = await handleDownloadRequest(request(link), context, null, network.fetcher);
  assert.equal(response.status, 302);
  assert.equal(response.headers.get('location'), old.assets[0].browser_download_url);
  assert.ok(network.calls.every(c => !c.url.endsWith('/latest')));
  const changed = link.replace('sha256=' + 'a'.repeat(64), 'sha256=' + 'c'.repeat(64));
  assert.equal((await handleDownloadRequest(request(changed), context, null, network.fetcher)).status, 503);
  const wrongSize = link.replace('size=1024', 'size=1023');
  assert.equal((await handleDownloadRequest(request(wrongSize), context, null, network.fetcher)).status, 503);
});

test('invalid mode/pins/methods are rejected before any upstream lookup', async () => {
  const network = transport();
  for (const path of ['/api/downloads?source=other', '/download/installer?version=v0.12.1', '/download/unknown']) {
    assert.equal((await handleDownloadRequest(request(path), context, null, network.fetcher)).status, 400);
  }
  assert.equal((await handleDownloadRequest(new Request('https://leigod.cmmuu.com/api/downloads', { method: 'POST' }), context, null, network.fetcher)).status, 405);
  assert.equal(network.calls.length, 0);
});

test('successful source metadata uses bounded cache; errors are not cached', async () => {
  const entries = new Map(), pending = [];
  const cache = { match: async key => entries.get(key.url)?.clone(), put: async (key, value) => { assert.equal(value.headers.get('cache-control'), 'public, max-age=300'); entries.set(key.url, value); } };
  const ctx = { waitUntil: p => pending.push(p) };
  const network = transport(fixture('gitee'));
  await handleDownloadRequest(request('/api/downloads?source=auto'), ctx, cache, network.fetcher);
  await Promise.all(pending);
  assert.equal(entries.size, 1);
  const before = network.calls.filter(c => c.url.includes('gitee')).length;
  await handleDownloadRequest(request('/api/downloads?source=auto'), ctx, cache, network.fetcher);
  assert.equal(network.calls.filter(c => c.url.includes('gitee')).length, before);
});
