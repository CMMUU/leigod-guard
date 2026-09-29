# 应用图标与折片背景

0.21.0 使用用户确认的“钛银＋钴蓝＋锐角折面”设计：分离的几何盾牌与加速箭头，以宽实块面和负形留白保持小尺寸辨识度。没有使用雷神或外星仔官方标识。

| 文件 | 用途 |
| --- | --- |
| `app-icon.png` | 256 × 256 RGBA，项目介绍 |
| `app-icon.ico` | 16、20、24、32、40、48、64、128、256 px，EXE、桌面／开始菜单快捷方式、安装器 |
| `app-icon-32.rgba` | 32 × 32 × 4 字节，系统托盘（含隐藏图标区域） |
| `app-icon-256.rgba` | 256 × 256 × 4 字节，窗口／任务栏／标题栏 |
| `aero-shards.png` | 1086 × 362 透明静态折片，仅用于首页右上空白区 |

RGBA 按行排列，顺序 R、G、B、A，透明度未经预乘。资源编译进程序，不依赖用户目录。ICO 九个帧由同一源图分别以 Lanczos 缩小，使用 32 位 PNG 帧；窗口和托盘也从同一源图独立缩小，不能仅替换展示 PNG。

源图 `docs/brand/aero-guard-source.png`（1254 × 1254）和 `docs/brand/aero-shards-source.png`（2172 × 724）于 2026-09-29 使用内置 imagegen 编辑已确认设计稿生成，随后仅进行尺寸和文件格式转换。旧 `accelerator-guard-source.png` 保留为历史来源。设计稿见 `docs/app-design/aero-home-concept.png`。

## 生成提示词

Logo：从已确认品牌板提取彩色主标，保持几何盾牌、箭头、钛银左上折面和钴蓝右侧折面及负形通道。单个居中方形标识，约 10% 留白，实际透明背景；不要文字、底板、投影、光晕、棋盘格或额外装饰，确保 16／32 px 仍可辨认。

背景：从已确认首页提取右上角金属折片流，3:1 横向透明图。钛银／钴蓝尖锐折面沿右上方向分布，左侧更细小稀疏，右侧稍大，左下留白；不要界面、文字、Logo、底色或光晕。

## 验证

Windows 流水线使用 `scripts/test-icons.ps1` 对实际 EXE、安装器和绿色 ZIP 内 EXE 逐帧读取并比对 ICO，不运行用户应用。原生界面测试确认标题栏使用新图标；发布前另检查 16／32 px 在浅、深底的可读性。`src/brand.rs` 的固定长度数组在编译时验证大小。

折片在首次显示时解码并缓存，窗口变窄时省略，不遮挡标题、选择框和按钮，不请求动画重绘。参考 Aero Shards 的折面语言，未引入 React Bits 运行时或动态 WebGPU 场景。

## 统一管理（0.22.0 起）

唯一维护源图仍是 `docs/brand/aero-guard-source.png`，`assets/brand.json` 统一声明输出路径与尺寸。`src/brand.rs` 是标题栏、窗口／任务栏、托盘的统一运行时引用；EXE 和安装器使用同一个 ICO。官网构建读取 `assets/app-icon.png`，后台 `server/static/logo.png` 由同一命令生成，不能手工替换。

```sh
python -m pip install -r scripts/brand-requirements.txt
python scripts/generate-brand.py
python scripts/generate-brand.py --check
```

生成依赖仅用于维护资源，不加入客户端或服务器运行时。`brand.generated.json` 记录源图、配置和所有产物 SHA-256；校验命令只需 Python 标准库。Windows CI、正式发布、Server CI 和官网构建均校验资源是否同步。修改源图时必须同时提交生成产物与记录。历史截图和旧版设计档案保留其当时的图标，不作为当前资源源头。

发布网站与后台时仍需正常部署静态资源；统一生成不代表已上传到线上。后台容器／可执行部署所需的静态文件已在仓库内，无需部署机器安装图片工具。


## Windows 升级后的图标刷新（0.22.2）

`egui::ViewportBuilder::with_icon` 在当前依赖中只设置窗口小图标。`brand::native_options` 统一窗口资源，`brand::install_taskbar_icon` 在实际窗口创建后通过 Windows 的 `WM_SETICON / ICON_BIG` 独立设置任务栏大图标，并在窗口销毁时回收持有的图标；实际 eframe 启动探针分别读取 `WM_GETICON / ICON_SMALL` 和 `ICON_BIG`，不能以离屏 UI 或 EXE 资源检查替代此项。

安装器为桌面、开始菜单、卸载入口显式指定 `branding/app-icon-<sha256>.ico`。`src/brand_shell.rs` 在后台为**直接指向当前 EXE**的已有桌面、产品开始菜单目录和已固定任务栏快捷方式更新 IconLocation；本地缓存名也取 ICO 的 SHA-256，避免旧 EXE 路径的图标缓存。保留名称、启动参数和工作目录，不创建新的固定项，不操作其他程序，不重启 Explorer 或清空系统缓存。绿色版无需在 ZIP 中携带额外 ICO，运行时从同一内嵌资源生成。
