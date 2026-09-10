const source = document.querySelector('#download-source');
const status = document.querySelector('#download-status');
const controls = document.querySelector('#download-controls');
const retry = document.querySelector('#download-retry');
let current;

function resetLinks(mode) {
  for (const link of document.querySelectorAll('[data-edition]')) {
    link.href = `/download/${link.dataset.edition}?source=${mode}`;
  }
  for (const links of document.querySelectorAll('[data-mirrors]')) links.replaceChildren();
}

async function refresh() {
  current?.abort();
  const controller = new AbortController(); current = controller;
  const timeout = setTimeout(() => controller.abort(), 12000);
  const mode = source.value;
  resetLinks(mode);
  retry.disabled = true;
  status.textContent = '正在检查可用正式版…';
  try {
    const response = await fetch(`/api/downloads?source=${mode}`, { signal: controller.signal, credentials: 'omit' });
    if (!response.ok) throw new Error('Release unavailable');
    const release = await response.json();
    if (current !== controller) return;
    status.textContent = `${release.partial ? '可用版本' : '最新正式版'} ${release.version} · ${release.source === 'gitee' ? 'Gitee 国内下载' : 'GitHub 下载'}${release.partial ? ' · 另一来源暂未完成检查' : ''}`;
    for (const link of document.querySelectorAll('[data-edition]')) link.href = release.downloads[link.dataset.edition].url;
    for (const holder of document.querySelectorAll('[data-mirrors]')) {
      const item = release.downloads[holder.dataset.mirrors];
      const label = document.createElement('span'); label.textContent = `${(item.size / 1048576).toFixed(1)} MB`;
      holder.append(label);
      for (const [name, href] of Object.entries(item.sources)) {
        if (name === release.source) continue;
        const link = document.createElement('a');
        link.href = href; link.target = '_blank'; link.rel = 'noopener';
        link.textContent = `${name === 'github' ? 'GitHub' : 'Gitee'} 同版本备用下载`;
        holder.append(link);
      }
    }
  } catch {
    if (current === controller) status.textContent = '暂未查到可用版本。可重新检查，或使用下方发布页入口。';
  } finally {
    clearTimeout(timeout);
    if (current === controller) retry.disabled = false;
  }
}

controls.hidden = false;
source.addEventListener('change', refresh);
retry.addEventListener('click', refresh);
refresh();
