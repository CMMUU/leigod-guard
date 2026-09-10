# 雷神守护项目网站

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

首页与下载区域提供安装版 EXE、绿色版 ZIP 两个直接下载按钮。`downloads.js` 请求本站 `/api/downloads`，由 `release-service.mjs` 比较两个来源的完整正式版，同版本优先 Gitee。Gitee 检查最近 100 条公开发布，GitHub 使用正式最新版接口；仅接受标准 `vX.Y.Z`、安装 EXE、绿色 ZIP 和 SHA256SUMS 全部就绪的发布。并行检查共用 8 秒期限，每个来源的成功结果在边缘缓存 5 分钟；来源失败显示检查不完整，不宣称已确认最新版。无需新增 token 或每次改写链接。

「仅 Gitee」「仅 GitHub」只检查所选平台。两种下载均通过 `/download/installer` 或 `/download/portable` 跳转到官方文件地址；显示版本后固定版本、文件大小和 SHA-256，备用入口只提供相同文件。浏览器开始文件传输后，网页无法可靠检测传输失败，因此保留手动同版本备用入口，不承诺浏览器自动换源或自动安装。服务异常保留发布页；无 JavaScript 时默认下载链接仍可由服务器解析。

只代理公开版本和小型校验清单，不代理安装包、不查询访客账户、不转发访客凭据。`functions/` 与 `dist/_routes.json` 只路由下载接口，正文、SEO 文件和图片保持静态服务。网站改版不需要新增 Windows 软件版本或重复发版。

验证：`node --test website/release-service.test.mjs`。包含来源不同步、单源、预发布、附件不完整、超时/离线、篡改地址和固定版本校验。Python 静态预览不运行 Functions；完整本地预览需在 `website` 目录执行 `npx wrangler pages dev dist --port 8877`。

## 搜索与 AI 资料维护

构建自动输出 `robots.txt`、`sitemap.xml`、`llms.txt`、`llms-full.txt`、`index.html.md` 及 IndexNow 验证文件。应用版本来自 Cargo，内容修改时间来自 Git 中最后一次相关修改，构建版本写入 HTML，避免用每次部署时间伪装内容更新。

正式域名允许收录。默认 `leigod-guard.pages.dev` 地址使用主机限定的 `X-Robots-Tag: noindex` 与 canonical 响应头，首页也指向正式 canonical；这不是域名重定向。Cloudflare 管理的 robots 内容会加在源文件之前，修改后应检查线上最终响应。搜索及回答检索允许抓取，训练爬虫维持禁止偏好。

GitHub Actions 的 `Website discovery` 在相关源码更新时运行：构建 → 等待相同内容在正式域名可用 → 通知 IndexNow。首次部署或网络短暂失败时会有限等待；失败后在 Actions 中手动重跑即可。也可在当前提交构建后运行 `node website/submit-indexnow.mjs --verify-only`，只检查线上部署而不提交。整个流程无需新增账户 token。

IndexNow 收到通知不等于已收录，也不代替 Google、百度的站点管理平台。验证、提交和后续检查方法见 [SEO 与 AI 可发现性维护](../docs/SEO.md)。
