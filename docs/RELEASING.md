# Windows 版本发布

本项目目前仅发布 Windows x64 便携包。正式自动构建目标为 `x86_64-pc-windows-msvc`；首个公开版本为 `0.5.0`，Git 标签为 `v0.5.0`。应用没有安装器、内置自动更新或代码签名。

## 版本与内容

每次发布需要保持下列版本一致：

- `Cargo.toml` 中 `[package].version`。
- `Cargo.lock` 中本项目条目的版本。
- `CHANGELOG.md` 中相应版本的实际变化与限制。
- Git 标签、Release 标题和 ZIP 文件名。

`v0.5.0` 的发布资产为：

| 资产 | 用途 |
| --- | --- |
| `leigod-guard-v0.5.0-windows-x64.zip` | 用户下载的便携程序包 |
| `SHA256SUMS.txt` | 发布文件的 SHA-256 校验值 |
| GitHub 自动提供的 `Source code` | 对应标签的源码归档，不是可运行程序 |

便携包应包含 `leigod-guard.exe`、运行所需的随包 DLL（如相应构建产物需要 `WebView2Loader.dll`）、使用文档、许可证和无账号数据的示例配置。不要把个人实际使用的 `config.toml`、token、验证码结果、日志、测试截图或原工作目录整体打包。

Microsoft WebView2 Runtime 不等同于 `WebView2Loader.dll`；包含后者也不能替代安装运行时。第三方运行时按各自许可与安装方式使用。

## 构建与检查

Windows 本机需准备 Rust、Visual Studio C++ Build Tools 和 Windows SDK。在仓库根目录执行：

```powershell
rustup target add x86_64-pc-windows-msvc
cargo build --release --locked --target x86_64-pc-windows-msvc
```

输出位于 `target\x86_64-pc-windows-msvc\release\`。保留 `Cargo.lock`，使用 `--locked` 防止发版过程中悄然更换依赖版本。构建成功只证明该构建环境可编译，不能证明外部账户接口或所有 Windows 环境均可用。

从仓库根目录运行打包脚本，生成便携 ZIP 和校验文件：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\package.ps1 -Target x86_64-pc-windows-msvc -OutputDirectory dist
```

脚本默认使用 MSVC 目标，运行 `cargo test --locked --target ...` 与 `cargo build --release --locked --target ...`，并支持 `CARGO_TARGET_DIR`。`dist` 是本地构建输出目录，不应提交到源码仓库。只有已经为同一份源码和同一目标完成测试与 release 构建时，才添加 `-SkipBuild` 复用已有产物。

发布前需要检查：

1. 代码与文档描述相符，版本号正确，仓库与归档均没有私人配置或凭据。
2. 构建与项目现有自动检查通过。
3. 从拟发布 ZIP 解压后启动，确认主窗口、隐藏到托盘、重新打开和正常退出可用。
4. 在有条件的实际环境中核对登录、验证码、游戏进程匹配、退出宽限期和暂停结果。涉及真实账户计时的检查应明确记录，未完成的部分不得写为通过。
5. 校验 ZIP 内容，计算哈希；Release 说明记录构建目标、验证环境和未验证的关键路径。

对关机、系统崩溃、断网和非公开接口的限制，沿用 README 的说明，不把一次成功测试描述为永久兼容保证。

## 发布标签与 Release

先将通过检查的源码和文档提交到仓库，确认提交已经推送。然后为该提交创建带说明的版本标签，例如：

```powershell
git tag -a v0.5.0 -m "Release v0.5.0"
git push origin v0.5.0
```

仓库的 [CI 工作流](https://github.com/CMMUU/leigod-guard/blob/v0.5.0/.github/workflows/ci.yml) 执行自动检查，[发布工作流](https://github.com/CMMUU/leigod-guard/blob/v0.5.0/.github/workflows/release.yml) 根据推送的 `v*` 标签构建与发布，也可通过 `workflow_dispatch` 指定已有标签。已有同名 Release 时，工作流会拒绝覆盖。

维护者应等待 GitHub Actions 的实际结果，确认 Release 已出现，并确认用户能够下载资产；只推送标签、只启动工作流或只上传 Actions artifact 不等于已经完成公开发版。

发布说明至少包括：本次版本变化、Windows x64 适用范围、下载资产名称、已有验证与局限，以及 README 的第三方项目声明。不要将“正式 Release”描述为雷神官方认可或认证。

## 本地备用构建

如果本机只配置了 Windows GNU Rust 工具链，可进行备用构建：

```powershell
cargo build --release --locked --target x86_64-pc-windows-gnu
```

该命令还需要对应的 GNU / MinGW 链接工具链，仅安装 Rust target 不一定足够。GNU 产物与 MSVC 产物是不同构建，依赖 DLL 和验证环境也可能不同。若实际发布使用了 GNU 产物，应在 Release 说明中如实标明，并从该 ZIP 启动检查，不能声称来自 MSVC 自动构建。

## 发版后

检查下载页中的文件名、版本、说明和校验值；记录完成状态。如果某版有缺陷，修复后增加版本号并发布新标签。已公开的版本资产与标签不应静默替换成其他构建，以便用户能够复现和核对来源。
