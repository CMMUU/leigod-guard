# 加速器守护项目网站

单页源码与 Windows 应用在同一仓库维护，设计稿位于 [`docs/website-design/`](../docs/website-design/README.md)。正文采用静态 HTML/CSS，不依赖前端框架或外部字体；下载区域通过一个本站脚本和 Pages Functions 查询公开正式版本。

访问地址：[leigod.cmmuu.com](https://leigod.cmmuu.com)。Cloudflare 默认地址：[leigod-guard.pages.dev](https://leigod-guard.pages.dev)。

## Cloudflare Pages 配置

| 配置项 | 值 |
| --- | --- |
| 项目名称 | `leigod-guard` |
| GitHub 仓库 | `CMMUU/leigod-guard` |
| 生产分支 | `main` |
| 根目录 | `website` |
| 框架预设 | None |
| 构建命令 | `node build.mjs` |
| 输出目录 | `dist` |
| 自定义域名 | `leigod.cmmuu.com` |

生产项目已使用上述构建设置接入 GitHub。管理入口为 Cloudflare 控制台 → Workers 和 Pages → `leigod-guard`。迁移部署时，在新项目的「自定义域」中添加域名，再按照控制台提示配置 DNS；不要直接覆盖已有的同名记录。

连接 Git 集成后，推送 `main` 会触发生产部署；可在 Pages 项目中查看构建日志与历史部署并回滚。构建无需雷神、Gitee 或 GitHub token。Gitee 镜像保存同一份源码，生产构建来源为 GitHub。

## 本地预览

在包含 Git 历史的仓库根目录执行（Git、Node.js 22 或更新版本）：

```sh
node website/build.mjs
python -m http.server 8877 --bind 127.0.0.1 --directory website/dist
```

浏览器打开 `http://127.0.0.1:8877/`。修改后重新构建并刷新；本地 Python 服务器不应用 Cloudflare 的 `_headers` 文件，也不使用 Pages 的自定义 404 页面。

## 修改内容

- `index.html`：正文、页内导航、下载与文档链接。
- `faq.json`：同时生成首页常见问题、FAQ 结构化数据和 Markdown 问答，避免多份说明不一致。
- `overview.md`：项目资料的 Markdown 模板；构建为 `/index.html.md`，与 README 规则摘录合并生成 `/llms-full.txt`。
- `styles.css`：桌面与移动端样式，支持减少动态效果的系统设置。
- `site.config.json`：生产域名、项目名和 IndexNow 站点所有权验证值，用于公开发现元数据；验证值不是账户访问令牌。
- `build.mjs`：构建静态输出，只重建本目录下的 `dist/`，不上传仓库其他内容。
- `404.html`：Cloudflare 上不存在路径的真实 404 响应，避免所有地址都返回首页 200。
- `submit-indexnow.mjs`：确认正式网站已经更新到当前构建且验证文件可读取后，向 IndexNow 通知首页地址。
- `wrangler.jsonc`：Pages 项目名与构建输出配置。

图标和应用截图来自仓库 `assets/app-icon.png`、`assets/ui-home.png`，版本来自 `Cargo.toml`，无需维护另一套副本。`dist/` 和工具缓存不提交。

## 自动下载入口

首页与下载区域默认使用 **下载中心**。`downloads.js` 请求本站 `/api/downloads`，`release-service.mjs` 从固定的 `https://downloads.cmmuu.com/api/catalog` 读取“加速器守护”公开条目，按正式版本号选择已发布版本，要求安装 EXE、绿色 ZIP 和 SHA256SUMS 全部就绪，并核对平台、大小、文件域、校验清单本身及两个成品的 SHA-256。下载按钮直接使用目录返回的 `files.cmmuu.com` 文件地址，不在源码保存版本号或文件 ID。

可长期使用两个固定入口：

- 安装版：`https://leigod.cmmuu.com/download/installer`
- 绿色版：`https://leigod.cmmuu.com/download/portable`

无 JavaScript 时这两个入口仍由服务端解析并跳转。下载中心成功结果最多缓存 60 秒；未发布、缺件、重复文件、大小/哈希不符时显示不可用，不偷偷换源或把旧版本冒充最新。文件字节由独立文件域传输，官网只查询公开目录和小型校验清单。

维护者在下载中心“加速器守护”分类上传并发布新版本的三个原始成品后，官网会自动跟随，无需修改链接或重新构建网站。版本字段统一为 `vX.Y.Z`，程序包平台为 `windows`、架构为 `x64`，保留原始文件名及 SHA256SUMS；同版同文件只保留一个公开条目。若将来接入正式发行导入，项目标识使用 `leigod-guard`。本次仅实现官网自动解析，未配置 GitHub 到下载中心的自动上传。

用户仍可手动选择「仅 Gitee」「仅 GitHub」，这两种模式只检查所选平台。旧分享链接 `source=auto` 保留 GitHub/Gitee 两源比较、同版本优先 Gitee 的行为；成功元数据缓存 5 分钟，检查期限 8 秒。备用来源的固定版本请求保留版本、文件大小和 SHA-256 约束。Windows 客户端的自动更新策略未改变。

不代理安装包、不查询访客账户、不转发访客凭据。`functions/` 与 `dist/_routes.json` 只路由下载接口，正文、SEO 文件和图片保持静态服务。网站修改无需新增 Windows 软件版本或重复发版。

验证：`node --test website/release-service.test.mjs`。覆盖下载中心版本推进、固定入口、直接文件链接、缺件/重复/草稿/错误平台/校验错误/异常跳转拒绝、60 秒缓存及原有严格单源和同版本备用逻辑。Python 静态预览不运行 Functions；完整本地预览可在 `website` 目录执行 `npx wrangler pages dev dist --port 8877`。

## 搜索与 AI 资料维护

构建自动输出 `robots.txt`、`sitemap.xml`、`llms.txt`、`llms-full.txt`、`index.html.md` 及 IndexNow 验证文件。应用版本来自 Cargo，内容修改时间来自 Git 中最后一次相关修改，构建版本写入 HTML，避免用每次部署时间伪装内容更新。

正式域名允许收录。默认 `leigod-guard.pages.dev` 地址使用主机限定的 `X-Robots-Tag: noindex` 与 canonical 响应头，首页也指向正式 canonical；这不是域名重定向。Cloudflare 管理的 robots 内容会加在源文件之前，修改后应检查线上最终响应。搜索及回答检索允许抓取，训练爬虫维持禁止偏好。

GitHub Actions 的 `Website discovery` 在相关源码更新时运行：构建 → 等待相同内容在正式域名可用 → 通知 IndexNow。首次部署或网络短暂失败时会有限等待；失败后在 Actions 中手动重跑即可。也可在当前提交构建后运行 `node website/submit-indexnow.mjs --verify-only`，只检查线上部署而不提交。整个流程无需新增账户 token。

IndexNow 收到通知不等于已收录，也不代替 Google、百度的站点管理平台。验证、提交和后续检查方法见 [SEO 与 AI 可发现性维护](../docs/SEO.md)。
