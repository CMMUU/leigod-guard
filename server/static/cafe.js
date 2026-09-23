// Settings are saved to the server; page timers never decide when to pause.
let cafeAccounts=[];
function cafeHtml(d){
 cafeAccounts=d.cafe||[];
 const ready=d.service.state==='ready';
 return `<div class="card section-gap"><div class="card-body"><p><strong>未安装守护的电脑也能使用云端时长上限。</strong> 默认连续未暂停 24 小时后，服务器尝试暂停并查询确认。可按账号设置 1–168 小时。</p><p class="muted">从服务器首次检测到正在计时开始，每分钟左右查询一次。检测到已暂停后结束本轮，再次恢复计时会重新开始。网页关闭不影响执行；查询长时间中断后会重新观察计时。</p><p class="muted mb-0">开启后，此账号的云端失联暂停改用网吧时长规则；到期即使仍在游戏也会暂停，请及时调整。已安装客户端的本机暂停仍按本机设置执行。${!ready?`<br><strong>服务器：${esc(remoteLabel(d.service.state))}，当前执行可能延后。</strong>`:''}</p></div></div>`+
 (cafeAccounts.length?cafeAccounts.map(c=>{
  const status=!c.enabled?'已关闭':c.credential!=='valid'?'需重新授权':c.job_state==='running'?'正在暂停并确认':c.job_state==='queued'?'到期等待执行':c.job_state==='unconfirmed'?'暂停未确认':c.observed_state==='paused'?'已暂停，等待下次计时':c.observed_state==='running'?'正在计时':'等待确认计时状态';
  return `<section class="card section-gap" data-cafe-account="${esc(c.id)}"><div class="card-header"><h2 class="card-title">${esc(c.label)}</h2>${badge(status,c.enabled&&c.credential==='valid'&&c.observed_state!=='unknown'?'green':'gray')}</div><div class="card-body"><p>连续未暂停上限：<strong>${c.max_hours} 小时</strong></p><p>本轮开始：${date(c.started_at)}<br>预计暂停：${c.enabled?date(c.deadline):'—'}<br>最近确认：${date(c.observed_at)}</p>${c.last_result?`<p class="muted">最近结果：${esc(remoteLabel(c.last_result))}</p>`:''}${c.credential!=='valid'?'<p class="text-danger">请在已安装守护的设备上重新授权加速器；凭据失效期间无法暂停。</p>':''}${c.user_id===me.id?`<button class="btn btn-primary" data-action="cafe-settings" data-id="${esc(c.id)}">设置网吧模式</button>`:`<p class="muted">所属用户：${esc(c.owner)} · 由用户本人设置</p>`}</div></section>`;
 }).join(''):`<section class="card">${empty('尚无已授权的加速器账号','先在家里的客户端开启对应加速器的服务器授权，再在此设置；网吧电脑无需下载守护。','shield')}</section>`);
}
function showCafeSettings(id){
 const c=cafeAccounts.find(x=>x.id===id);
 if(!c||c.user_id!==me.id)throw new Error('账号设置已变化，请刷新');
 modal(`<h2>网吧模式</h2><p>${esc(c.label)}</p><form id="cafe-form"><input type="hidden" name="account_id" value="${esc(c.id)}"><input type="hidden" name="revision" value="${c.revision}"><label class="form-check mb-3"><input class="form-check-input" name="enabled" type="checkbox" ${c.enabled?'checked':''}><span class="form-check-label">开启并授权云端持续保存凭据、按时长自动暂停此账号</span></label><div class="mb-3"><label class="form-label" for="cafe-hours">连续未暂停多久后自动暂停（小时）</label><input class="form-control" id="cafe-hours" name="max_hours" type="number" min="1" max="168" step="1" required value="${c.max_hours}"><small class="muted">默认 24 小时，可设置 1–168 小时。</small></div><p class="muted text-small">修改时长按本轮开始时间重新计算；缩短到已过去的时长，可能立即触发暂停。此设置独立于设备授权，关闭网页或设备保护不会关闭网吧模式；请在此取消勾选并保存。关闭后设备失联保护需收到新心跳才恢复。</p><div id="cafe-error"></div><div class="actions"><button class="btn" data-action="close" type="button">取消</button><button class="btn btn-primary" type="submit">保存设置</button></div></form>`);
}
async function saveCafeSettings(form,p){
 const max_hours=Number(p.max_hours);
 if(!Number.isInteger(max_hours)||max_hours<1||max_hours>168)throw new Error('请输入 1–168 的整数小时');
 await api(`/remote/cafe/${p.account_id}`,{enabled:form.elements.enabled.checked,max_hours,revision:Number(p.revision)});
}
