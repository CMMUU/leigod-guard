//! 内嵌 WebView2 的极验 v4 人机验证窗口。
//!
//! 设计要点：
//! - 必须在独立线程创建 tao 事件循环（主线程已被 egui/eframe 占用）；
//! - tao 的 `EventLoop::run` 返回 `!`，退出时会 `process::exit` 杀掉整个进程，
//!   因此必须使用 `run_return`（桌面平台可用）让窗口关闭后线程自然结束；
//! - 验证结果通过 `shared.captcha_result` 回传给 UI 线程：
//!   `Some(json)` = 验证通过（极验凭据 JSON），`Some("")` = 用户关闭窗口取消。
use crate::shared::Shared;
use std::sync::{Arc, Mutex};

/// 弹出极验 v4 人机验证窗口（异步，立即返回）。
pub fn spawn(captcha_id: String, server_status: i64, shared: Arc<Mutex<Shared>>) {
    std::thread::spawn(move || {
        if let Err(e) = run_window(&captcha_id, server_status, &shared) {
            if let Ok(mut s) = shared.lock() {
                s.log(&format!("人机验证窗口启动失败: {e}"));
                // 视为取消，避免 UI 一直等待
                s.captcha_result = Some(String::new());
            }
        }
    });
}

/// 子进程模式入口：弹验证窗口，把结果写入结果文件后退出（返回进程退出码）。
/// 与主程序进程隔离：主程序里 eframe/wgpu 与 WebView2 窗口共存时，会被系统中
/// 注入的第三方 DLL（输入法/加速器/OSD 钩子）带崩，独立进程则稳定。
pub fn run_subprocess(captcha_id: String, server_status: i64) -> i32 {
    let shared = Arc::new(Mutex::new(Shared::default()));
    spawn(captcha_id, server_status, Arc::clone(&shared));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    loop {
        if let Ok(mut s) = shared.lock() {
            if let Some(res) = s.captcha_result.take() {
                let _ = write_result_file(&res);
                return 0;
            }
        }
        if std::time::Instant::now() > deadline {
            let _ = write_result_file(""); // 超时按取消处理
            return 2;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

fn result_file_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("leigod-guard").join("captcha-result.txt"))
}

fn write_result_file(res: &str) -> std::io::Result<()> {
    let Some(p) = result_file_path() else {
        return Ok(());
    };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, res)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

/// 以独立子进程弹出验证窗口（立即返回）
pub fn spawn_subprocess(captcha_id: &str, server_status: i64) -> Result<(), String> {
    validate_captcha_id(captcha_id)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // 清掉可能的旧结果文件，避免读到上次残留
    if let Some(p) = result_file_path() {
        let _ = std::fs::remove_file(p);
    }
    std::process::Command::new(exe)
        .arg("--captcha")
        .arg(captcha_id)
        .arg(server_status.to_string())
        .spawn()
        .map_err(|e| format!("启动验证子进程失败: {e}"))?;
    Ok(())
}

/// 非阻塞读取验证结果文件（读到即删）
pub fn take_result_file() -> Option<String> {
    let p = result_file_path()?;
    let content = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    Some(content)
}

fn run_window(
    captcha_id: &str,
    server_status: i64,
    shared: &Arc<Mutex<Shared>>,
) -> Result<(), String> {
    // captcha_id 也可来自命令行；禁止把任意文本拼进页面脚本。
    validate_captcha_id(captcha_id)?;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::platform::windows::EventLoopBuilderExtWindows;
    use tao::window::WindowBuilder;
    use wry::WebViewBuilder;

    crate::ui::dbglog("[captcha] run_window enter (v4)");

    // IPC 回调写入的验证结果
    let result: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // 用 localhost 小服务器承载页面：about:blank/null 起源会让极验 SDK 抛跨域异常，
    // 有真实 http 起源后才能正常工作。
    let page_url = serve_page(page_html(captcha_id, server_status))?;
    crate::ui::dbglog(&format!("[captcha] page served: {page_url}"));

    // any_thread(true)：tao 默认禁止在非主线程建事件循环（Windows 上其实是安全的）
    let mut event_loop = EventLoopBuilder::new().with_any_thread(true).build();
    crate::ui::dbglog("[captcha] event loop built");
    let window = WindowBuilder::new()
        .with_title("人机验证 - 雷神守护")
        .with_inner_size(tao::dpi::LogicalSize::new(400.0, 480.0))
        .with_resizable(false)
        .build(&event_loop)
        .map_err(|e| format!("创建窗口失败: {e}"))?;
    crate::ui::dbglog("[captcha] window created");

    let result_ipc = Arc::clone(&result);
    let webview = WebViewBuilder::new()
        .with_url(&page_url)
        .with_ipc_handler(move |req| {
            let body = req.body();
            // 验证结果和 JS 错误可能含临时凭据，仅记录固定事件名称。
            if let Some(message) = ipc_log_message(body) {
                crate::ui::dbglog(message);
            }
            if let Some(rest) = body.strip_prefix("ok:") {
                if let Ok(mut r) = result_ipc.lock() {
                    *r = Some(rest.to_string());
                }
            }
        })
        .build(&window)
        .map_err(|e| format!("创建 WebView2 失败（需要系统装有 WebView2 运行时）: {e}"))?;
    crate::ui::dbglog("[captcha] webview created");

    let shared_ev = Arc::clone(shared);
    event_loop.run_return(move |event, _, control_flow| {
        // 短超时轮询：既响应窗口事件，也能及时取到 IPC 结果
        *control_flow = ControlFlow::WaitUntil(
            std::time::Instant::now() + std::time::Duration::from_millis(120),
        );
        if let Ok(r) = result.lock() {
            if let Some(v) = &*r {
                if let Ok(mut s) = shared_ev.lock() {
                    s.captcha_result = Some(v.clone());
                    s.log("人机验证已通过，正在重试之前的操作…");
                }
                *control_flow = ControlFlow::Exit;
                return;
            }
        }
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            if let Ok(mut s) = shared_ev.lock() {
                s.captcha_result = Some(String::new());
                s.log("人机验证窗口被关闭，操作已取消");
            }
            *control_flow = ControlFlow::Exit;
        }
    });
    // 事件循环结束后线程的消息泵已停，此时 drop WebView2/窗口可能访问失效的
    // 消息循环导致进程崩溃；验证窗口生命周期很短，直接泄漏这几百 KB 更安全。
    crate::ui::dbglog("[captcha] run_return exited");
    std::mem::forget(webview);
    std::mem::forget(window);
    Ok(())
}

fn validate_captcha_id(captcha_id: &str) -> Result<(), String> {
    if captcha_id.len() == 32 && captcha_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("人机验证配置无效，请更新应用后重试".into())
    }
}

fn ipc_log_message(body: &str) -> Option<&'static str> {
    match body {
        "log:page-start" => Some("[captcha] page started"),
        "log:geetest4-ready" => Some("[captcha] component ready"),
        "log:captcha-visible" => Some("[captcha] challenge visible"),
        "log:captcha-error" => Some("[captcha] challenge error"),
        body if body.starts_with("log:js-error:") => Some("[captcha] script error"),
        body if body.starts_with("ok:") => Some("[captcha] verification result received"),
        _ => None,
    }
}

/// 生成承载极验 v4 组件的页面（官网当前设置 is_off_geetest_login=1 即用 v4）。
/// captcha_id 为官网硬编码的极验 v4 captchaId；server_status 来自 geetest/config，
/// 验证成功后需随结果一并回传。字符串直接替换注入，避免 format! 的花括号转义问题。
fn page_html(captcha_id: &str, server_status: i64) -> String {
    let tpl = r##"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  html,body{margin:0;height:100%;background:#1c1f2b;color:#e8e8ec;
    font-family:"Microsoft YaHei",sans-serif;display:flex;flex-direction:column;
    align-items:center;justify-content:center}
  h3{margin:0 0 6px;font-size:16px;font-weight:600}
  .tip{font-size:12px;color:#9aa;margin-bottom:18px}
  #captcha{width:300px;min-height:44px}
  .err{color:#f66;font-size:12px;margin-top:14px;max-width:320px;text-align:center}
</style>
<script src="https://static.geetest.com/v4/gt4.js"></script>
</head>
<body>
<h3>请完成人机验证</h3>
<div class="tip">验证通过后本窗口会自动关闭并重试操作</div>
<div id="captcha">加载中…若长时间无反应请检查网络后重开窗口</div>
<div class="err" id="err"></div>
<script>
window.onerror = function (m) { try { window.ipc.postMessage("log:js-error:" + m); } catch (e) {} };
window.ipc.postMessage("log:page-start");
if (typeof initGeetest4 === "undefined") {
  window.ipc.postMessage("log:js-error:gt4.js 未加载");
  document.getElementById("captcha").innerText = "验证组件加载失败，请检查网络后重试";
} else {
  initGeetest4({
    captchaId: "__CID__",
    product: "bind",
    hideSuccess: true,
    https: true
  }, function (c) {
    window.ipc.postMessage("log:geetest4-ready");
    c.onReady(function () { window.ipc.postMessage("log:captcha-visible"); });
    c.onSuccess(function () {
      var r = c.getValidate();
      r.server_status = __SS__;
      window.ipc.postMessage("ok:" + JSON.stringify(r));
    });
    c.onError(function (e) {
      window.ipc.postMessage("log:captcha-error");
      document.getElementById("err").innerText = "验证组件出错，请关闭窗口重试";
    });
    c.showCaptcha();
  });
}
</script>
</body>
</html>"##;
    tpl.replace("__CID__", captcha_id)
        .replace("__SS__", &server_status.to_string())
}

/// 在 127.0.0.1 随机端口起一个极简 HTTP 服务，承载验证页面。
/// 返回页面 URL；服务线程在进程存活期内有效（窗口生命周期很短，可接受）。
fn serve_page(html: String) -> Result<String, String> {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| format!("本地端口绑定失败: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            // 读完请求头即可（不解析路径，任何 GET 都返回同一页面）
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let body = html.as_bytes();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes());
            let _ = s.write_all(body);
            let _ = s.flush();
        }
    });
    Ok(format!("http://127.0.0.1:{port}/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_diagnostics_never_include_verification_values() {
        let sensitive = "ok:{\"pass_token\":\"private-test-value\"}";
        assert_eq!(
            ipc_log_message(sensitive),
            Some("[captcha] verification result received")
        );
        assert_eq!(
            ipc_log_message("log:js-error:private-test-value"),
            Some("[captcha] script error")
        );
        assert_eq!(ipc_log_message("private-test-value"), None);
    }

    #[test]
    fn captcha_ids_cannot_inject_script() {
        assert!(validate_captcha_id(crate::leigod_api::GEETEST_V4_CAPTCHA_ID).is_ok());
        assert!(validate_captcha_id(crate::leigod_api::GEETEST_V4_CAPTCHA_ID_PWD).is_ok());
        assert!(validate_captcha_id("\"};alert(1);//").is_err());
        assert!(validate_captcha_id(&"界".repeat(32)).is_err());
        assert!(validate_captcha_id("").is_err());
    }
}
