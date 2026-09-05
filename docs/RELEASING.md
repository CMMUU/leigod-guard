# Windows 版本发布

本项目目前仅发布 Windows x64 版本，**每次正式发布同时提供安装 EXE 和绿色免安装 ZIP**。普通用户优先使用安装版。正式自动构建目标为 `x86_64-pc-windows-msvc`，使用静态 C/C++ 运行库链接。首个公开版本为 `0.5.0`，从 `0.5.1` 开始提供安装版，从 `0.6.0` 开始支持应用内检查和更新。安装程序与应用尚未进行 Windows 代码签名。

## 版本与内容

每次发布需要保持下列版本一致：

- `Cargo.toml` 中 `[package].version`。
- `Cargo.lock` 中本项目条目的版本。
- `CHANGELOG.md` 中相应版本的实际变化与限制。
- Git 标签、Release 标题、安装程序版本以及 EXE / ZIP 文件名。

`v0.8.3` 的发布资产为：

| 资产 | 用途 |
| --- | --- |
| `leigod-guard-v0.8.3-windows-x64-setup.exe` | 推荐下载；Windows x64 安装程序 |
| `leigod-guard-v0.8.3-windows-x64.zip` | 绿色免安装程序包 |
| `SHA256SUMS.txt` | 发布文件的 SHA-256 校验值 |
| GitHub 自动提供的 `Source code` | 对应标签的源码归档，不是可运行程序 |

便携包应包含 `leigod-guard.exe`、运行所需的随包 DLL（如相应构建产物需要 `WebView2Loader.dll`）、使用文档、许可证和无账号数据的示例配置。不要把个人实际使用的 `config.toml`、token、验证码结果、日志、测试截图或原工作目录整体打包。

绿色免安装版仅省去安装向导；应用仍将配置及日志存入 `%APPDATA%\leigod-guard\`，可选自启会写入当前用户的注册表。发布说明不能将其描述为所有数据随程序移动或完全不写注册表的版本。

当前版本延续独立的 `strategy.startup_grace_secs`，新旧配置缺少该值时均默认 180 秒；`strategy.pause_on_startup` 仍默认开启。Release 说明应明确：自动暂停开启、启动时名单非空且进程名全部有效，从首次成功且没有名单游戏的检查开始连续等待，等待结束仍无名单游戏时尝试暂停；首页或托盘可延后至少 10 分钟。发现游戏后结束启动检查，正常退出宽限期仍默认 90 秒。已完成的启动检查不因后续手动开启加速而重新启动，工具不能识别雷神加速按钮的点击。启动时无名单、任一进程名无效或相关开关关闭时跳过；之后添加名单或开启选项不会补做本次已经跳过的启动检查。游戏预设只帮助填写进程名，不能描述为已通过真实游戏、账户接口或反作弊测试。

0.8.1 新增默认关闭的游戏加加屏蔽选项，保存后须从托盘完全退出雷神守护并重新打开才生效。它使用 Windows 严格的进程 DLL 加载策略，只保护雷神守护进程，不关闭或修改游戏加加；同时也会阻止其他不属于 Microsoft、Microsoft Store 或 WHQL 信任范围的 DLL，可能影响其他 OSD、录屏或输入法插件。当前验证范围为 GamePP SDK 1.2.1615.625 与 NVIDIA 显卡环境，AMD、Intel 显卡环境尚未覆盖，发布说明不得扩大兼容性结论。

安装器默认安装到当前用户的 `%LOCALAPPDATA%\Programs\LeigodGuard\`，无需管理员权限；提供开始菜单和可选桌面快捷方式及 Windows 应用列表卸载入口。安装时迁移已有的本工具自启路径，未开启自启则不创建；卸载仅移除指向本安装目录的自启项。更新和卸载保留 `%APPDATA%\leigod-guard\` 中的配置、账户数据和日志。

安装包包含微软 WebView2 Runtime 安装引导程序。安装时检测到运行时缺失，才运行它联网补装。**这不是包含完整 WebView2 Runtime 的离线安装包。** Microsoft WebView2 Runtime 不等同于 `WebView2Loader.dll`；便携包中的加载器也不能代替运行时。第三方运行时按各自许可与安装方式使用，卸载本工具时不卸载共享的 WebView2 Runtime。

## 应用内更新与发布约定

「关于与更新」提供 GitHub / Gitee（国内）来源选择、手动检查，以及默认关闭的「启动时自动检查更新」选项。旧配置默认 GitHub。GitHub 使用本仓库的 `releases/latest` 公开接口；Gitee 从公开列表最近 100 条中比较版本号，并读取所选 Release 的附件列表，不使用会受旧说明编辑影响的 `latest` 排序。仅接受比当前内置版本更高的正式 `v主版本.次版本.修订版本`，不安装草稿或预发布版本。切换来源会清除旧结果，需重新检查；检查本身不下载或应用更新，用户点击「下载并更新」后才开始。

更新器按当前发行方式选择安装 EXE 或绿色 ZIP。必须继续使用上表的固定命名格式，并在同一 Release 中同时提供两个文件与 `SHA256SUMS.txt`。校验表对每个资产只保留一条精确文件名记录；程序检查文件大小、SHA-256，并在 GitHub 提供资产摘要时与其核对。Gitee 的附件 API 没有独立摘要，使用同一 Release 的校验表，并核对 Release 与附件 API 的名称、编号、下载地址及大小。检查、校验和程序下载均使用同一来源，不跨来源自动回退。不要在上传完整资产前公开正式版本，也不要为“更新”复用旧版本号。哈希文件和程序来自同一发布来源，完整性校验不能替代代码签名。

更新会关闭当前应用并在完成后重新打开，保留 `%APPDATA%\leigod-guard\`。监控在此期间短暂停止；更新辅助进程本身不请求暂停，但重新启动的应用会执行正常启动策略，当前版本默认等待 180 秒后按上述条件决定是否暂停，不能承诺更新后不触发暂停。已经消耗的时长无法追回。绿色版应更新原程序目录，安装版应复用已有安装路径，不能在用户点击更新后悄然切换发行方式。游戏加加屏蔽默认关闭；已开启时，更新后的重新启动应继续采用严格加载策略。0.5.x 用户首次迁移到支持应用内更新的版本，需要手动下载安装 EXE 或新版 ZIP。

更新器通过当前用户的本项目卸载记录核对运行目录，识别安装版；其他正常程序目录使用绿色 ZIP。应用位于网络共享、符号链接或目录联接下，或主程序被改名时，不支持自动应用更新。下载暂存位于 `%LOCALAPPDATA%\LeigodGuard\updates\`，绿色版事务备份位于原目录的 `.leigod-update-*\`；当前不会自动清理这些缓存和备份，清理说明见 [隐私说明](PRIVACY.md)。

每次发版都要更新 README 中两种成品的直达下载链接，并在更新说明中写明实际行为。所选来源网络不可用、校验失败、程序目录权限不足或文件占用时，界面应保留可理解的错误信息和两个来源的手动下载入口。0.8.1 及更早版本仅支持 GitHub；其用户若无法访问 GitHub，需先从 Gitee 手动升级一次，才能在应用中切换到 Gitee。

## 构建与检查

Windows 本机需准备 Rust、Visual Studio C++ Build Tools 和 Windows SDK。在仓库根目录执行：

```powershell
rustup target add x86_64-pc-windows-msvc
cargo build --release --locked --target x86_64-pc-windows-msvc
```

输出位于 `target\x86_64-pc-windows-msvc\release\`。仓库的 MSVC 配置启用了 `crt-static`，以免普通用户另行安装 Visual C++ 运行库；发布时不要用环境变量覆盖并丢失该配置。保留 `Cargo.lock`，使用 `--locked` 防止发版过程中悄然更换依赖版本。构建成功只证明该构建环境可编译，不能证明外部账户接口或所有 Windows 环境均可用。

### 应用图标

`assets/app-icon.ico` 包含 16 至 256 像素的多尺寸图标。`build.rs` 在 MSVC 目标使用 Windows SDK 的 `rc.exe`，在 GNU 目标使用 MinGW `windres`；必要时用 `LEIGOD_RC` / `LEIGOD_WINDRES` 指定对应可执行文件。无需新增 Cargo 依赖。窗口和托盘分别内嵌 256 与 32 像素 RGBA 数据，安装器通过 `SetupIconFile` 使用相同 ICO。资源来源、生成提示词和更新方式见 [assets/README.md](../assets/README.md)。

CI 和发布构建都会运行 `scripts/test-icons.ps1`，以资源读取模式核对实际 release EXE、安装器和 ZIP 内 EXE 的图标帧，不启动应用。缓存键包含 `build.rs` 和 `assets/**`，更新资源后不能沿用旧 EXE 的图标结果。

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
5. 在有条件的实际环境中核对登录、验证码、游戏进程匹配、退出宽限期和暂停结果。等工具完成暂停后，在雷神官方微信小程序登录同一账号并下拉刷新，记录刷新后的计时状态；核对前不要手动点击小程序的暂停按钮，以免混淆自动暂停结果。工具的成功提示不能单独作为通过依据。涉及真实账户计时的检查应明确记录，未完成的部分不得写为通过。
6. 校验 EXE / ZIP 内容及哈希；Release 说明记录构建目标、验证环境和未验证的关键路径。
7. 对更新功能，检查默认关闭与旧配置兼容、手动检查和启动检查、版本比较、无新版、网络失败、缺失资产及哈希不匹配等分支；核对安装版和绿色版分别保持发行方式、更新后版本正确且用户配置不变。升级失败或文件被占用时还应检查恢复结果，不能仅凭下载成功认定自动更新通过。
8. 对启动暂停，核对新旧配置默认 180 秒、与退出宽限期独立、延后 10 分钟及重复点击、检测到游戏后结束、已完成检查不重启、关闭选项、空名单或无效进程名、进程检查失败重计连续等待、暂停失败冷却，以及慢登录后的延后请求复核；失败和取消请求不能写作成功。常用游戏预设应与最终源码清单一致，保留自定义文件名和运行进程选择入口。更新集成测试使用的模拟程序不验证新版本正常启动后的真实账户暂停行为，应单独说明这一限制。
9. 对游戏加加屏蔽，核对新旧配置均默认关闭；开启后从托盘完全退出雷神守护并重新打开，确认策略页显示生效，且雷神守护进程不再加载 `GPP64.dll`、`GameTracker64.dll`、`shade64.dll` 或 `GPP_VKLayer64.dll`。关闭后也须完全退出并重新打开，并核对普通启动仍正常。测试记录应说明严格策略还会拒绝其他不属于 Microsoft、Microsoft Store 或 WHQL 信任范围的 DLL，不得描述为只过滤游戏加加。当前仅覆盖 GamePP SDK 1.2.1615.625 与 NVIDIA 显卡环境；AMD、Intel 显卡环境须列为未验证，也不得声称本功能关闭、修改或兼容所有版本的游戏加加。

CI 和发布工作流都会运行 `scripts/test-updater.ps1`。该集成测试仅允许在全新的 GitHub 托管 Windows runner 上执行：用实际发布程序启动更新辅助进程，以不读取账号、不启动监控的下一版本测试程序，验证安装版与绿色版的完整应用更新、父进程退出等待、配置和自启保留，以及重新启动。测试包只位于 runner 临时目录，不进入正式发布资产；日志单独上传为 `updater-test-logs-*`。本机离线单元测试不运行这些安装操作。

0.8.3 起，Windows x64 MSVC 单元测试还会编译一份无害、无签名的临时 DLL，并通过实际保护代码创建独立子进程：先确认该 DLL 在普通进程中可加载，再验证受保护子进程在准备代码执行前已继承错误处理标志、仍拒绝 DLL 加载，并在 10 秒内结束。该测试不会启动应用界面、读取账号或请求暂停；它验证 Windows 加载行为，不替代游戏加加实际注入、显卡和录屏软件组合的兼容性检查。

对关机、系统崩溃、断网和非公开接口的限制，沿用 README 的说明，不把一次成功测试描述为永久兼容保证。

## 发布标签与 Release

先将通过检查的源码和文档提交到仓库，确认提交已经推送。然后为该提交创建带说明的版本标签，例如：

```powershell
git tag -a v0.8.3 -m "Release v0.8.3"
git push origin v0.8.3
```

仓库的 [CI 工作流](https://github.com/CMMUU/leigod-guard/blob/v0.8.3/.github/workflows/ci.yml) 执行自动检查，[发布工作流](https://github.com/CMMUU/leigod-guard/blob/v0.8.3/.github/workflows/release.yml) 根据推送的 `v*` 标签构建安装版、绿色免安装版和校验文件并发布，也可通过 `workflow_dispatch` 指定已有标签。已有同名 Release 时，工作流会拒绝覆盖。

维护者应等待 GitHub Actions 的实际结果，确认 Release 已出现，并确认用户能够下载资产；只推送标签、只启动工作流或只上传 Actions artifact 不等于已经完成公开发版。GitHub 发版完成后，还应检查下方 Gitee 同步任务；GitHub 成功不能代替 Gitee 的发布结果。

发布说明至少包括：本次版本变化、Windows x64 适用范围、两种成品的下载入口、首推的安装 EXE 名称、按需联网补装 WebView2 的条件、应用内更新的入口与监控中断提示、启动暂停默认值及更新后重启的影响、游戏加加屏蔽的默认状态、严格范围和显卡验证边界、已有验证与局限，以及 README 的第三方项目声明。不要将“正式 Release”描述为雷神官方认可或认证。

## 自动同步到 Gitee

**只在 GitHub 构建和发版一次，Gitee 复用完全相同的成品，不需要另行编译或每次手动发布。** Git 同步本身只传递代码、分支和标签，Release 说明及二进制附件由 [Sync GitHub to Gitee 工作流](../.github/workflows/sync-gitee.yml) 另行自动复制。

一次性配置：

1. 在 [Gitee 私人令牌设置](https://gitee.com/profile/personal_access_tokens) 创建允许读写 `cmmuu/leigod-guard` 仓库、Release 与附件的令牌。
2. 在 [GitHub 仓库 Actions Secrets](https://github.com/CMMUU/leigod-guard/settings/secrets/actions) 添加名称为 `GITEE_TOKEN` 的仓库密钥，将令牌直接保存在密钥值中。不要写入源码、工作流正文、Issue 或聊天。GitHub 侧使用 Actions 自动提供的只读 `github.token`。
3. 打开 GitHub Actions 的 **Sync GitHub to Gitee**，手动运行一次并确认成功，补齐已有代码、标签和公开版本。之后正常在 GitHub 发版即可；令牌过期或权限变化时才需要更新此密钥。

触发与完成条件：

- 推送 `main` 自动同步代码与标签。**Publish Windows release** 成功后自动同步所有已公开的 Release、说明、安装 EXE、绿色 ZIP 和 `SHA256SUMS.txt`；直接发布或编辑 Release、手动运行也可触发。构建失败不触发发布同步。
- 使用 `workflow_run` 接续发布工作流，覆盖 GitHub 内置令牌创建 Release 不会再触发普通 `release` 工作流的情况。同步任务串行运行，只允许本项目的可信主分支脚本操作固定的目标仓库。
- Gitee 无草稿 Release API；新版本先以**预发布**状态创建，上传并重新下载每个附件核对大小与 SHA-256，全部通过后才按 GitHub 状态转为正式版。同步中的预发布可能在网页可见，应用会忽略它。GitHub 原本为预发布时仍保持预发布。
- 现有同名附件先下载校验，一致则复用；冲突或重复名称会报错，不替换或删除文件。分支和标签采用非强制推送，遇到冲突停止；不会删除 Gitee 独有引用。
- 缺少或失效的 `GITEE_TOKEN`、网络失败、附件冲突都会使同步任务失败，GitHub 已成功的发布不受影响。修复后在 Actions 重跑，任务会核对已经同步的内容并继续。两个来源版本可能暂时不同，应用不会降级。

应用用户不需要令牌，也不运行同步脚本。Gitee 附件下载允许官方 `foruda.gitee.com` 分发域名；若 Gitee 更换域名，应先核实来源再同时更新发布脚本和应用的下载白名单。`GITEE_ASSET_HOSTS` 仓库变量只扩展维护脚本的允许域名，不会修改已发布应用。

离线测试使用 `python -m unittest discover -s scripts -p test_sync_gitee.py`，覆盖仓库归属、隐私、凭据隔离、附件校验、冲突处理、断点重跑与完整上传后转正式版。最终仍需在公开页面及下载 API 确认两个版本号、三个文件名、文件大小和 SHA-256 一致；任务启动或代码同步成功不能代替附件同步验证。

## 本地备用构建

如果本机只配置了 Windows GNU Rust 工具链，可进行备用构建：

```powershell
cargo build --release --locked --target x86_64-pc-windows-gnu
```

该命令还需要对应的 GNU / MinGW 链接工具链，仅安装 Rust target 不一定足够。GNU 产物与 MSVC 产物是不同构建，依赖 DLL 和验证环境也可能不同。若实际发布使用了 GNU 产物，应在 Release 说明中如实标明，并从该 ZIP 启动检查，不能声称来自 MSVC 自动构建。

## 发版后

检查下载页中的文件名、版本、说明和校验值；记录完成状态。如果某版有缺陷，修复后增加版本号并发布新标签。已公开的版本资产与标签不应静默替换成其他构建，以便用户能够复现和核对来源。
