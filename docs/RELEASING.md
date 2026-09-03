# Windows 版本发布

本项目目前仅发布 Windows x64 版本，**每次正式发布同时提供安装 EXE 和绿色免安装 ZIP**。普通用户优先使用安装版。正式自动构建目标为 `x86_64-pc-windows-msvc`，使用静态 C/C++ 运行库链接。首个公开版本为 `0.5.0`，从 `0.5.1` 开始提供安装版，从 `0.6.0` 开始支持应用内检查和更新。安装程序与应用尚未进行 Windows 代码签名。

## 版本与内容

每次发布需要保持下列版本一致：

- `Cargo.toml` 中 `[package].version`。
- `Cargo.lock` 中本项目条目的版本。
- `CHANGELOG.md` 中相应版本的实际变化与限制。
- Git 标签、Release 标题、安装程序版本以及 EXE / ZIP 文件名。

`v0.6.1` 的发布资产为：

| 资产 | 用途 |
| --- | --- |
| `leigod-guard-v0.6.1-windows-x64-setup.exe` | 推荐下载；Windows x64 安装程序 |
| `leigod-guard-v0.6.1-windows-x64.zip` | 绿色免安装程序包 |
| `SHA256SUMS.txt` | 发布文件的 SHA-256 校验值 |
| GitHub 自动提供的 `Source code` | 对应标签的源码归档，不是可运行程序 |

便携包应包含 `leigod-guard.exe`、运行所需的随包 DLL（如相应构建产物需要 `WebView2Loader.dll`）、使用文档、许可证和无账号数据的示例配置。不要把个人实际使用的 `config.toml`、token、验证码结果、日志、测试截图或原工作目录整体打包。

绿色免安装版仅省去安装向导；应用仍将配置及日志存入 `%APPDATA%\leigod-guard\`，可选自启会写入当前用户的注册表。发布说明不能将其描述为所有数据随程序移动或完全不写注册表的版本。

安装器默认安装到当前用户的 `%LOCALAPPDATA%\Programs\LeigodGuard\`，无需管理员权限；提供开始菜单和可选桌面快捷方式及 Windows 应用列表卸载入口。安装时迁移已有的本工具自启路径，未开启自启则不创建；卸载仅移除指向本安装目录的自启项。更新和卸载保留 `%APPDATA%\leigod-guard\` 中的配置、账户数据和日志。

安装包包含微软 WebView2 Runtime 安装引导程序。安装时检测到运行时缺失，才运行它联网补装。**这不是包含完整 WebView2 Runtime 的离线安装包。** Microsoft WebView2 Runtime 不等同于 `WebView2Loader.dll`；便携包中的加载器也不能代替运行时。第三方运行时按各自许可与安装方式使用，卸载本工具时不卸载共享的 WebView2 Runtime。

## 应用内更新与发布约定

「关于与更新」提供手动检查，以及默认关闭的「启动时自动检查更新」选项。检查使用本仓库的 GitHub `releases/latest` 公开接口，仅接受比当前内置版本更高的正式 `v主版本.次版本.修订版本`，不安装草稿或预发布版本。检查本身不下载或应用更新，用户点击「下载并更新」后才开始。

更新器按当前发行方式选择安装 EXE 或绿色 ZIP。必须继续使用上表的固定命名格式，并在同一 Release 中同时提供两个文件与 `SHA256SUMS.txt`。校验表对每个资产只保留一条精确文件名记录；程序检查文件大小、SHA-256，并在 GitHub 提供资产摘要时与其核对。不要在上传完整资产前将草稿公开为最新版本，也不要为“更新”复用旧版本号。哈希文件和程序来自同一发布来源，完整性校验不能替代代码签名。

更新会关闭当前应用并在完成后重新打开，保留 `%APPDATA%\leigod-guard\`。监控在此期间短暂停止；升级操作不自动请求暂停雷神账户计时。绿色版应更新原程序目录，安装版应复用已有安装路径，不能在用户点击更新后悄然切换发行方式。0.5.x 用户首次迁移到支持应用内更新的版本，需要手动下载安装 EXE 或新版 ZIP。

更新器通过当前用户的本项目卸载记录核对运行目录，识别安装版；其他正常程序目录使用绿色 ZIP。应用位于网络共享、符号链接或目录联接下，或主程序被改名时，不支持自动应用更新。下载暂存位于 `%LOCALAPPDATA%\LeigodGuard\updates\`，绿色版事务备份位于原目录的 `.leigod-update-*\`；当前不会自动清理这些缓存和备份，清理说明见 [隐私说明](PRIVACY.md)。

每次发版都要更新 README 中两种成品的直达下载链接，并在更新说明中写明实际行为。GitHub 网络不可用、校验失败、程序目录权限不足或文件占用时，界面应保留可理解的错误信息和手动下载入口。

## 构建与检查

Windows 本机需准备 Rust、Visual Studio C++ Build Tools 和 Windows SDK。在仓库根目录执行：

```powershell
rustup target add x86_64-pc-windows-msvc
cargo build --release --locked --target x86_64-pc-windows-msvc
```

输出位于 `target\x86_64-pc-windows-msvc\release\`。仓库的 MSVC 配置启用了 `crt-static`，以免普通用户另行安装 Visual C++ 运行库；发布时不要用环境变量覆盖并丢失该配置。保留 `Cargo.lock`，使用 `--locked` 防止发版过程中悄然更换依赖版本。构建成功只证明该构建环境可编译，不能证明外部账户接口或所有 Windows 环境均可用。

制作安装版还需准备 **Inno Setup 6.4 或更高版本**。安装打包脚本会检查编译器，但不自动安装它。在仓库根目录运行：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\installer.ps1 -Target x86_64-pc-windows-msvc -OutputDirectory dist
```

该命令先调用 `scripts/package.ps1`，运行测试与 release 编译，按白名单生成便携 ZIP；然后从 ZIP 暂存内容构建安装 EXE，最后生成包含 EXE 和 ZIP 两条记录的 `SHA256SUMS.txt`。默认使用 MSVC 目标，支持 `CARGO_TARGET_DIR`。`dist` 是本地构建输出目录，不应提交到源码仓库。只有已经为同一份源码和同一目标完成测试与 release 构建时，才添加 `-SkipBuild` 复用已有产物。

默认从微软地址下载 WebView2 安装引导程序，并检查 Authenticode 签名有效且签名者为 Microsoft Corporation。可通过 `-InnoCompiler` 指定 `ISCC.exe`，或通过 `-BootstrapperPath` 复用已下载的微软引导程序；复用文件仍需通过签名检查。例如：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\installer.ps1 `
  -InnoCompiler 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe' `
  -BootstrapperPath 'C:\BuildTools\MicrosoftEdgeWebview2Setup.exe' `
  -OutputDirectory dist -SkipBuild
```

引导程序的微软签名不等于本项目安装包已经签名。它会在最终用户安装时按需联网下载运行时，不能据此宣称安装版支持全离线安装。

WebView2 Bootstrapper / Runtime 使用独立的微软许可，不能套用 SDK、Rust 绑定或本项目的 MIT 许可。官方来源与随包条款位于 [`licenses/webview2-runtime/`](../licenses/webview2-runtime/README.md)；安装向导展示 `INSTALLER-LICENSE.txt` 中的本项目及微软条款和 SmartScreen 告知。更新该组件时，应核对微软当前分发条款，并保持随包文档和安装向导内容一致。

只需制作便携版时，可单独运行 `scripts/package.ps1 -Target x86_64-pc-windows-msvc -OutputDirectory dist`。这会生成仅包含 ZIP 校验值的校验文件；正式发布两个资产时，应最后运行安装打包脚本，以获得完整的双文件校验表。

发布前需要检查：

1. 代码与文档描述相符，版本号正确，仓库与归档均没有私人配置或凭据。
2. 构建与项目现有自动检查通过。
3. 使用拟发布 EXE 核对安装路径、快捷方式、应用版本及卸载入口；确认卸载保留用户数据。从便携 ZIP 解压后也检查程序与文档是否齐全。
4. 在可用的测试环境核对 WebView2 已安装及缺失两条路径；缺失路径还应检查联网补装失败时的提示和重试行为。启动应用后核对主窗口、隐藏到托盘、重新打开和正常退出。0.6.1 起，托盘「退出」应保存配置后直接退出，不弹确认或因退出请求暂停；账户页「退出程序」仍保留确认流程。更新准备期间的托盘退出应提示更新进行中。
5. 在有条件的实际环境中核对登录、验证码、游戏进程匹配、退出宽限期和暂停结果。涉及真实账户计时的检查应明确记录，未完成的部分不得写为通过。
6. 校验 EXE / ZIP 内容及哈希；Release 说明记录构建目标、验证环境和未验证的关键路径。
7. 对更新功能，检查默认关闭与旧配置兼容、手动检查和启动检查、版本比较、无新版、网络失败、缺失资产及哈希不匹配等分支；核对安装版和绿色版分别保持发行方式、更新后版本正确且用户配置不变。升级失败或文件被占用时还应检查恢复结果，不能仅凭下载成功认定自动更新通过。

CI 和发布工作流都会运行 `scripts/test-updater.ps1`。该集成测试仅允许在全新的 GitHub 托管 Windows runner 上执行：用实际发布程序启动更新辅助进程，以不读取账号、不启动监控的下一版本测试程序，验证安装版与绿色版的完整应用更新、父进程退出等待、配置和自启保留，以及重新启动。测试包只位于 runner 临时目录，不进入正式发布资产；日志单独上传为 `updater-test-logs-*`。本机离线单元测试不运行这些安装操作。

对关机、系统崩溃、断网和非公开接口的限制，沿用 README 的说明，不把一次成功测试描述为永久兼容保证。

## 发布标签与 Release

先将通过检查的源码和文档提交到仓库，确认提交已经推送。然后为该提交创建带说明的版本标签，例如：

```powershell
git tag -a v0.6.1 -m "Release v0.6.1"
git push origin v0.6.1
```

仓库的 [CI 工作流](https://github.com/CMMUU/leigod-guard/blob/v0.6.1/.github/workflows/ci.yml) 执行自动检查，[发布工作流](https://github.com/CMMUU/leigod-guard/blob/v0.6.1/.github/workflows/release.yml) 根据推送的 `v*` 标签构建安装版、绿色免安装版和校验文件并发布，也可通过 `workflow_dispatch` 指定已有标签。已有同名 Release 时，工作流会拒绝覆盖。

维护者应等待 GitHub Actions 的实际结果，确认 Release 已出现，并确认用户能够下载资产；只推送标签、只启动工作流或只上传 Actions artifact 不等于已经完成公开发版。

发布说明至少包括：本次版本变化、Windows x64 适用范围、两种成品的下载入口、首推的安装 EXE 名称、按需联网补装 WebView2 的条件、应用内更新的入口与监控中断提示、已有验证与局限，以及 README 的第三方项目声明。不要将“正式 Release”描述为雷神官方认可或认证。

## 本地备用构建

如果本机只配置了 Windows GNU Rust 工具链，可进行备用构建：

```powershell
cargo build --release --locked --target x86_64-pc-windows-gnu
```

该命令还需要对应的 GNU / MinGW 链接工具链，仅安装 Rust target 不一定足够。GNU 产物与 MSVC 产物是不同构建，依赖 DLL 和验证环境也可能不同。若实际发布使用了 GNU 产物，应在 Release 说明中如实标明，并从该 ZIP 启动检查，不能声称来自 MSVC 自动构建。

## 发版后

检查下载页中的文件名、版本、说明和校验值；记录完成状态。如果某版有缺陷，修复后增加版本号并发布新标签。已公开的版本资产与标签不应静默替换成其他构建，以便用户能够复现和核对来源。
