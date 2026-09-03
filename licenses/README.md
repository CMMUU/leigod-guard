# 补充第三方许可与来源

此目录补充 [THIRD_PARTY_NOTICES.txt](../THIRD_PARTY_NOTICES.txt) 中仅靠 crate 根目录扫描不能取得的上游许可证、版权信息和随包资产声明。它不替代各组件的原始授权，也不把第三方组件重新授权为本项目的 MIT 许可证。

除明确标注的元数据提取说明外，许可与声明文件保留上游原文。Rust crate 的对应提交依据已锁定版本发布包中的 `.cargo_vcs_info.json`；字体文件来自 `epaint_default_fonts 0.31.1` 发布包。收集日期：2026-09-03。

## egui 组件与内嵌字体

`ecolor`、`eframe`、`egui`、`egui-wgpu`、`egui-winit`、`emath`、`epaint` 均为 **0.31.1**，声明 `MIT OR Apache-2.0`。`epaint_default_fonts 0.31.1` 的 Rust 代码沿用该授权，字体另有授权。

原始仓库提交：[`emilk/egui@1669e52a7ccfc3489c1b0999b9ed48894a0b3887`](https://github.com/emilk/egui/tree/1669e52a7ccfc3489c1b0999b9ed48894a0b3887)。

| 本地原文 | 精确上游来源 |
| --- | --- |
| [egui LICENSE-MIT](egui-0.31.1/LICENSE-MIT) | [LICENSE-MIT](https://raw.githubusercontent.com/emilk/egui/1669e52a7ccfc3489c1b0999b9ed48894a0b3887/LICENSE-MIT) |
| [egui LICENSE-APACHE](egui-0.31.1/LICENSE-APACHE) | [LICENSE-APACHE](https://raw.githubusercontent.com/emilk/egui/1669e52a7ccfc3489c1b0999b9ed48894a0b3887/LICENSE-APACHE) |
| [Hack 字体声明](epaint-default-fonts-0.31.1/Hack-Regular.txt) | [fonts/Hack-Regular.txt](https://raw.githubusercontent.com/emilk/egui/1669e52a7ccfc3489c1b0999b9ed48894a0b3887/crates/epaint_default_fonts/fonts/Hack-Regular.txt) |
| [Noto Emoji 字体 OFL](epaint-default-fonts-0.31.1/OFL.txt) | [fonts/OFL.txt](https://raw.githubusercontent.com/emilk/egui/1669e52a7ccfc3489c1b0999b9ed48894a0b3887/crates/epaint_default_fonts/fonts/OFL.txt) |
| [Ubuntu 字体 UFL](epaint-default-fonts-0.31.1/UFL.txt) | [fonts/UFL.txt](https://raw.githubusercontent.com/emilk/egui/1669e52a7ccfc3489c1b0999b9ed48894a0b3887/crates/epaint_default_fonts/fonts/UFL.txt) |
| [emoji-icon-font MIT](epaint-default-fonts-0.31.1/emoji-icon-font-mit-license.txt) | [fonts/emoji-icon-font-mit-license.txt](https://raw.githubusercontent.com/emilk/egui/1669e52a7ccfc3489c1b0999b9ed48894a0b3887/crates/epaint_default_fonts/fonts/emoji-icon-font-mit-license.txt) |

[FONT-METADATA.txt](epaint-default-fonts-0.31.1/FONT-METADATA.txt) 记录从原始 TTF 的 `name` 表中提取的版权、字体名及许可信息。Hack 原文同时包含 Source Foundry、Bitstream Vera 及 DejaVu 的声明；这些内容没有简化成单一 MIT 标记。字体二进制本身不在本目录另行分发。

程序还可在运行时读取 Windows 已安装的中文字体；这些系统字体不复制进本项目发布包，其权利归属与许可保持不变。

## AccessKit

| crate 与版本 | crate 声明 |
| --- | --- |
| `accesskit 0.17.1` | `MIT OR Apache-2.0` |
| `accesskit_consumer 0.26.0` | `MIT OR Apache-2.0` |
| `accesskit_windows 0.24.1` | `MIT OR Apache-2.0` |
| `accesskit_winit 0.23.1` | `Apache-2.0` |

这四个发布包对应 [`AccessKit/accesskit@405d578cdfd8496ee79020d24cfaac8c29a8a48f`](https://github.com/AccessKit/accesskit/tree/405d578cdfd8496ee79020d24cfaac8c29a8a48f)。保留以下原文，包括上游说明的 Chromium 衍生部分：

| 本地原文 | 精确上游来源 |
| --- | --- |
| [LICENSE-MIT](accesskit-405d578/LICENSE-MIT) | [上游 MIT](https://raw.githubusercontent.com/AccessKit/accesskit/405d578cdfd8496ee79020d24cfaac8c29a8a48f/LICENSE-MIT) |
| [LICENSE-APACHE](accesskit-405d578/LICENSE-APACHE) | [上游 Apache](https://raw.githubusercontent.com/AccessKit/accesskit/405d578cdfd8496ee79020d24cfaac8c29a8a48f/LICENSE-APACHE) |
| [LICENSE.chromium](accesskit-405d578/LICENSE.chromium) | [上游 Chromium 声明](https://raw.githubusercontent.com/AccessKit/accesskit/405d578cdfd8496ee79020d24cfaac8c29a8a48f/LICENSE.chromium) |
| [AUTHORS](accesskit-405d578/AUTHORS) | [上游作者名单](https://raw.githubusercontent.com/AccessKit/accesskit/405d578cdfd8496ee79020d24cfaac8c29a8a48f/AUTHORS) |

## WebView2 Rust 绑定与微软 SDK

Rust 绑定声明为 MIT，分别保留精确发布提交的原文：

| crate 与版本 | 本地原文 | 精确上游来源 |
| --- | --- | --- |
| `webview2-com 0.38.2`、`webview2-com-sys 0.38.2` | [LICENSE](webview2-rs-0.38.2/LICENSE) | [`b74dc5e2b394044bea5191052868ce7a106c202c/LICENSE`](https://raw.githubusercontent.com/wravery/webview2-rs/b74dc5e2b394044bea5191052868ce7a106c202c/LICENSE) |
| `webview2-com-macros 0.8.1` | [LICENSE](webview2-com-macros-0.8.1/LICENSE) | [`dffa41a8a46d3f5565eefbff2de57d38d399f158/LICENSE`](https://raw.githubusercontent.com/wravery/webview2-rs/dffa41a8a46d3f5565eefbff2de57d38d399f158/LICENSE) |

**微软 `WebView2Loader` 的许可独立于 Rust 绑定的 MIT。** 使用的 SDK 为 [Microsoft.Web.WebView2 1.0.3650.58](https://www.nuget.org/packages/Microsoft.Web.WebView2/1.0.3650.58)。以下文件直接从该版 [微软 NuGet 包](https://api.nuget.org/v3-flatcontainer/microsoft.web.webview2/1.0.3650.58/microsoft.web.webview2.1.0.3650.58.nupkg) 提取：

- [LICENSE.txt](webview2-sdk-1.0.3650.58/LICENSE.txt)：该 SDK 包的原始许可证；也可查阅 [NuGet 许可页面](https://www.nuget.org/packages/Microsoft.Web.WebView2/1.0.3650.58/License)。
- [NOTICE.txt](webview2-sdk-1.0.3650.58/NOTICE.txt)：保留 SDK 整包所附第三方声明，不代表所列组件均由本工具直接调用。
- [Microsoft.Web.WebView2.nuspec](webview2-sdk-1.0.3650.58/Microsoft.Web.WebView2.nuspec)：原始包元数据，记录版本和 `LICENSE.txt` 引用。

核对结果：`webview2-com-sys 0.38.2` 发布包中的 x64 `WebView2Loader.dll` 与上述 NuGet 的 `build/native/x64/WebView2Loader.dll` 的 SHA-256 相同：

```text
8427B1FC58EC707813E5C0A51EB5D69397BB333250A7B891BE4D3B123F1E0F1C
```

`wry 0.53.5` 自身的 MIT / Apache 原文已由根目录扫描收入 `THIRD_PARTY_NOTICES.txt`。用户另行安装的 WebView2 Runtime 不随本项目的 ZIP 分发，其授权仍由微软提供。

## 其他缺失的 crate 根目录许可证

其他精确版本依赖的原文、逐项来源和特殊情况记录见 [misc/SOURCES.md](misc/SOURCES.md)。只有许可文本和来源说明进入本目录；上游源码归档、下载缓存与构建产物不进入发布包。

## 更新方式

变更 `Cargo.lock` 后，应重新按实际 Windows 目标的依赖图检查各版本许可证与嵌入资产。对于缺少独立许可文件的发布包，应检查精确提交的官方仓库，保留许可声明的依据；不要从无关项目复制同名许可证来冒充上游原文。
