//! Explicitly enabled account coordination. Failure never falls back to direct pause.
use crate::cycle_protocol::{CycleRequest, CycleView};
use crate::platform_api::{Api, DeviceBinding, Provider};
use crate::{config::Config, game_lifecycle::Phase, shared::Shared};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Context {
    pub binding: DeviceBinding,
    pub run: i64,
    pub revision: i64,
    pub enrolled: bool,
    pub token_digest: String,
}

#[derive(Default)]
pub struct Runtime {
    pub context: Option<Context>,
    pub required: bool,
    pub view: CycleView,
    pub executor_at: Option<Instant>,
    pub sequence: i64,
    pub next_poll: Option<Instant>,
    pub local_policy: Option<(u32, u32)>,
}

pub fn required(shared: &Mutex<Shared>) -> bool {
    shared.lock().map_or(true, |s| s.cycle.required)
}

pub fn frame(
    shared: &Mutex<Shared>,
    kind: Provider,
    worker: bool,
) -> Option<(Context, CycleRequest)> {
    let mut s = shared.lock().ok()?;
    let c = s.cycle.context.clone().filter(|c| c.enrolled)?;
    if worker {
        s.cycle.executor_at = Some(Instant::now());
    }
    s.cycle.sequence = s.cycle.sequence.saturating_add(1);
    let o = s.game_monitor.as_ref()?.latest();
    let phase = match o.phase {
        Phase::Running => "running",
        Phase::Launching => "launching",
        Phase::Absent => "absent",
        Phase::Unknown => "unknown",
    };
    let age = |at: Option<Instant>| at.map_or(86400, |at| at.elapsed().as_secs().min(86400) as u32);
    Some((
        c.clone(),
        CycleRequest {
            op: "observe".into(),
            provider: kind.as_str().into(),
            run_generation: c.run,
            grant_revision: c.revision,
            sequence: s.cycle.sequence,
            cycle_id: s.cycle.view.cycle_id.clone(),
            phase: phase.into(),
            game_generation: o.generation,
            observer_age: if o.phase == Phase::Unknown {
                86400
            } else {
                age(Some(o.at))
            },
            executor_age: age(s.cycle.executor_at),
            prepare_seconds: o.remaining(Instant::now()).min(600) as u32,
            ..Default::default()
        },
    ))
}

pub fn observe(shared: &Mutex<Shared>, kind: Provider, api: &Api) {
    if let Some((c, q)) = frame(shared, kind, false) {
        if let Ok(v) = api.cycle(&c.binding, &q) {
            publish(shared, &c, v);
        }
    }
}
fn publish(shared: &Mutex<Shared>, c: &Context, v: CycleView) {
    if let Ok(mut s) = shared.lock() {
        if s.cycle.context.as_ref().is_some_and(|n| {
            n.run == c.run && n.revision == c.revision && n.binding.device_id == c.binding.device_id
        }) {
            s.cycle.view = v;
        }
    }
}
fn path(kind: Provider) -> std::path::PathBuf {
    Config::path().with_file_name(format!("cycle-outbox-{}.dat", kind.as_str()))
}
fn save(kind: Provider, q: &CycleRequest) -> Result<(), String> {
    use std::io::Write;
    let data = crate::dpapi::protect(&serde_json::to_string(q).map_err(|_| "周期记录无效")?)
        .map_err(|_| "周期记录无法加密")?;
    let target = path(kind);
    let temp = target.with_extension("tmp");
    let mut f = std::fs::File::create(&temp).map_err(|_| "无法保存暂停意图")?;
    f.write_all(data.as_bytes())
        .and_then(|_| f.sync_all())
        .map_err(|_| "无法保存暂停意图")?;
    drop(f);
    std::fs::rename(temp, target).map_err(|_| "无法提交暂停意图".into())
}
fn flush(kind: Provider, api: &Api, c: &Context) -> Result<(), String> {
    let raw = match std::fs::read_to_string(path(kind)) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("暂停结果待办无法读取".into()),
    };
    let plain = crate::dpapi::unprotect(&raw).map_err(|_| "暂停结果待办无法解密")?;
    let q: CycleRequest = serde_json::from_str(&plain).map_err(|_| "暂停结果待办无效")?;
    api.cycle(&c.binding, &q)
        .map_err(|_| "等待云端确认上次暂停结果")?;
    std::fs::remove_file(path(kind)).map_err(|_| "暂停确认记录无法清理".into())
}
fn random_id() -> Result<String, String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(|_| "无法生成动作标识")?;
    Ok(b.iter().map(|v| format!("{v:02x}")).collect())
}

pub fn step(shared: &Arc<Mutex<Shared>>, cfg: &Arc<Mutex<Config>>, kind: Provider) {
    if let Err(e) = step_inner(shared, cfg, kind) {
        if let Ok(mut s) = shared.lock() {
            s.set_status(&e);
        }
    }
}
fn step_inner(
    shared: &Arc<Mutex<Shared>>,
    cfg: &Arc<Mutex<Config>>,
    kind: Provider,
) -> Result<(), String> {
    let config = cfg.lock().map_err(|_| "无法读取守护策略")?.clone();
    let _permit = config.permit(kind).ok_or("等待守护对象切换确认")?;
    let api = Api::new().map_err(|_| "等待云端连接")?;
    let (c, mut q) = frame(shared, kind, true).ok_or("云端周期协同等待授权确认")?;
    flush(kind, &api, &c)?;
    let token = shared
        .lock()
        .map_err(|_| "无法读取账号")?
        .token
        .clone()
        .ok_or("请登录当前加速器账号")?;
    let credential = crate::platform_api::RemoteCredential {
        account_token: token.clone(),
        device_id: if kind == Provider::Etalien {
            config.etalien.device_id.clone()
        } else {
            String::new()
        },
        paused_state: if kind == Provider::Etalien {
            config.etalien.paused_state.unwrap_or(0)
        } else {
            0
        },
    };
    if crate::platform_device::token_digest(&credential) != c.token_digest {
        return Err("账号已变化，等待云端重新授权确认".into());
    }
    let poll = shared
        .lock()
        .map_err(|_| "无法读取周期")?
        .cycle
        .next_poll
        .is_none_or(|at| at <= Instant::now());
    if poll {
        if let Ok(mut s) = shared.lock() {
            s.cycle.next_poll = Some(Instant::now() + Duration::from_secs(30));
        }
        refresh(shared, &config, kind, &token);
    }
    {
        let s = shared.lock().map_err(|_| "无法读取计时观察")?;
        q.billing_age = s
            .account_info_received_at
            .map_or(86400, |at| at.elapsed().as_secs().min(86400) as u32);
        q.billing_paused = paused(&s, &config, kind);
    }
    let desired = (
        config.strategy.startup_grace_secs.min(3600) as u32,
        config.strategy.grace_secs.min(3600) as u32,
    );
    let changed = shared
        .lock()
        .is_ok_and(|s| s.cycle.local_policy.is_some_and(|old| old != desired));
    if changed {
        let mut policy = q.clone();
        policy.op = "policy".into();
        policy.startup_seconds = desired.0;
        policy.exit_seconds = desired.1;
        api.cycle(&c.binding, &policy)
            .map_err(|_| "策略变更等待云端确认")?;
    }
    if let Ok(mut s) = shared.lock() {
        s.cycle.local_policy = Some(desired);
    }
    let v = api
        .cycle(&c.binding, &q)
        .map_err(|_| "等待云端同步，自动暂停暂缓")?;
    q.cycle_id = v.cycle_id.clone();
    publish(shared, &c, v.clone());
    if !changed && desired != (v.startup_seconds, v.exit_seconds) {
        if let Ok(mut local) = cfg.lock() {
            local.strategy.startup_grace_secs = v.startup_seconds.into();
            local.strategy.grace_secs = v.exit_seconds.into();
            local.save()?;
        }
        if let Ok(mut s) = shared.lock() {
            s.cycle.local_policy = Some((v.startup_seconds, v.exit_seconds));
        }
    }
    let manual = {
        let mut s = shared.lock().map_err(|_| "无法读取操作")?;
        s.running_games = s
            .game_monitor
            .as_ref()
            .map(|m| m.latest().running)
            .unwrap_or_default();
        s.set_status(&format!(
            "{}{}",
            v.message,
            if v.remaining_seconds > 0 {
                format!("（{} 秒）", v.remaining_seconds)
            } else {
                String::new()
            }
        ));
        s.manual_cmd.take().is_some()
    };
    if !manual && (!config.strategy.enabled || v.mode == "cafe" || v.state != "ready") {
        return Ok(());
    }
    if q.cycle_id.is_none() {
        return Err("尚未确认计时状态，暂停暂缓".into());
    }
    verify(
        shared,
        cfg,
        &config,
        kind,
        &token,
        manual,
        q.game_generation,
    )?;
    q.op = "claim".into();
    q.manual = manual;
    q.request_id = random_id()?;
    let permit = api
        .cycle(&c.binding, &q)
        .map_err(|_| "云端执行许可未确认")?;
    let Some(action) = permit.action_id else {
        return Err("其他设备、待确认动作或云端保护尚未满足".into());
    };
    q.action_id = Some(action);
    // Crash before/after dispatch is represented as unknown, never as success.
    q.op = "complete".into();
    q.result = "unknown".into();
    save(kind, &q)?;
    let mut dispatch = q.clone();
    dispatch.op = "dispatch".into();
    api.cycle(&c.binding, &dispatch)
        .map_err(|_| "执行许可复核未确认，等待结果核对")?;
    let dispatched = Instant::now();
    let mut attempted = false;
    let result = if kind == Provider::Leigod {
        verify(
            shared,
            cfg,
            &config,
            kind,
            &token,
            manual,
            q.game_generation,
        )
        .and_then(|_| {
            if dispatched.elapsed() > Duration::from_secs(5) {
                return Err("许可已过期".into());
            }
            attempted = true;
            crate::leigod_api::pause(&token)
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    } else {
        crate::etalien_api::pause(
            &token,
            &config.etalien.device_id,
            config.etalien.paused_state.ok_or("外星仔尚未校准")?,
            Duration::from_secs(18),
            || {
                verify(
                    shared,
                    cfg,
                    &config,
                    kind,
                    &token,
                    manual,
                    q.game_generation,
                )?;
                if dispatched.elapsed() > Duration::from_secs(10) {
                    return Err("执行许可已过期".into());
                }
                attempted = true;
                Ok(())
            },
        )
        .map(|_| ())
    };
    refresh(shared, &config, kind, &token);
    let confirmed = shared.lock().is_ok_and(|s| {
        s.account_info_received_at
            .is_some_and(|at| at >= dispatched)
            && paused(&s, &config, kind) == Some(true)
    });
    q.result = if confirmed {
        "confirmed"
    } else if !attempted {
        "cancelled"
    } else if result
        .as_ref()
        .is_err_and(|e| e.contains("400855") || e.contains("体验时间"))
    {
        "restricted"
    } else {
        "unknown"
    }
    .into();
    save(kind, &q)?;
    flush(kind, &api, &c)?;
    if let Ok(mut s) = shared.lock() {
        s.log(if confirmed {
            "本轮暂停已由官方查询确认，已同步云端；继续观察下一次计时"
        } else {
            "本轮暂停尚未确认，已同步云端核对，不重复发送"
        });
        s.manual_pause_result = manual.then_some(confirmed);
    }
    Ok(())
}
fn verify(
    shared: &Mutex<Shared>,
    cfg: &Mutex<Config>,
    expected: &Config,
    kind: Provider,
    token: &str,
    manual: bool,
    generation: u64,
) -> Result<(), String> {
    let config = cfg.lock().map_err(|_| "策略不可用")?.clone();
    if !config.is_active(kind)
        || config.selection_generation != expected.selection_generation
        || (!manual && !config.strategy.enabled)
    {
        return Err("守护对象或策略已变化".into());
    }
    let monitor = {
        let s = shared.lock().map_err(|_| "状态不可用")?;
        if s.token.as_deref() != Some(token) {
            return Err("账号已变化".into());
        }
        s.game_monitor.clone().ok_or("游戏观察不可用")?
    };
    let observation = monitor.observe_now(&config)?;
    if !manual && (observation.phase != Phase::Absent || observation.generation != generation) {
        return Err("发现游戏或准备活动，取消本次自动暂停".into());
    }
    Ok(())
}
pub fn paused(s: &Shared, c: &Config, kind: Provider) -> Option<bool> {
    if kind == Provider::Leigod {
        s.account_info
            .as_ref()
            .and_then(crate::leigod_api::account_paused)
    } else {
        s.etalien_info
            .as_ref()
            .and_then(|i| i.pause_state)
            .and_then(|state| {
                if Some(state) == c.etalien.paused_state {
                    Some(true)
                } else if state == 0 {
                    Some(false)
                } else {
                    None
                }
            })
    }
}
pub fn refresh(shared: &Mutex<Shared>, c: &Config, kind: Provider, token: &str) {
    if kind == Provider::Leigod {
        if let Ok(info) = crate::leigod_api::user_info(token) {
            if let Ok(mut s) = shared.lock() {
                s.set_account_info(token, info);
            }
        }
    } else if let Ok(info) = crate::etalien_api::info(token, &c.etalien.device_id) {
        if let Ok(mut s) = shared.lock() {
            s.set_etalien_info(token, info);
        }
    }
}
