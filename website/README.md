# 雷神守护项目网站

单页源码与 Windows 应用在同一仓库维护，设计稿位于 [`docs/website-design/`](../docs/website-design/README.md)。网站采用真实 HTML/CSS，不依赖前端框架、运行时脚本或外部字体。

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

网站链接到两个平台的公开发布页，由用户选择安装版 EXE 或绿色版 ZIP；不额外托管安装包、不查询访客账户。网站改版不需要新增 Windows 软件版本或重复发版。

## 搜索与 AI 资料维护

构建自动输出 `robots.txt`、`sitemap.xml`、`llms.txt`、`llms-full.txt`、`index.html.md` 及 IndexNow 验证文件。应用版本来自 Cargo，内容修改时间来自 Git 中最后一次相关修改，构建版本写入 HTML，避免用每次部署时间伪装内容更新。

正式域名允许收录。默认 `leigod-guard.pages.dev` 地址使用主机限定的 `X-Robots-Tag: noindex` 与 canonical 响应头，首页也指向正式 canonical；这不是域名重定向。Cloudflare 管理的 robots 内容会加在源文件之前，修改后应检查线上最终响应。搜索及回答检索允许抓取，训练爬虫维持禁止偏好。

GitHub Actions 的 `Website discovery` 在相关源码更新时运行：构建 → 等待相同内容在正式域名可用 → 通知 IndexNow。首次部署或网络短暂失败时会有限等待；失败后在 Actions 中手动重跑即可。也可在当前提交构建后运行 `node website/submit-indexnow.mjs --verify-only`，只检查线上部署而不提交。整个流程无需新增账户 token。

IndexNow 收到通知不等于已收录，也不代替 Google、百度的站点管理平台。验证、提交和后续检查方法见 [SEO 与 AI 可发现性维护](../docs/SEO.md)。
