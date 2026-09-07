# 雷神守护项目网站

单页源码与 Windows 应用在同一仓库维护，设计稿位于 [`docs/website-design/`](../docs/website-design/README.md)。网站采用真实 HTML/CSS，不依赖前端框架、运行时脚本或外部字体。

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

在 Cloudflare 的 Workers 和 Pages 中连接仓库并使用上述构建设置。首次部署成功后，在项目的「自定义域」中添加域名，再按照控制台提示配置 DNS。不要直接覆盖已有的同名 DNS 记录。

连接 Git 集成后，推送 `main` 会触发生产部署；可在 Pages 项目中查看构建日志与历史部署并回滚。构建无需雷神、Gitee 或 GitHub token。Gitee 镜像保存同一份源码，生产构建来源为 GitHub。

## 本地预览

在仓库根目录执行（Node.js 22 或更新版本）：

```sh
node website/build.mjs
python -m http.server 8877 --bind 127.0.0.1 --directory website/dist
```

浏览器打开 `http://127.0.0.1:8877/`。修改后重新构建并刷新；本地 Python 服务器不应用 Cloudflare 的 `_headers` 文件。

## 修改内容

- `index.html`：正文、页内导航、下载与文档链接。
- `styles.css`：桌面与移动端样式，支持减少动态效果的系统设置。
- `site.config.json`：生产域名与项目名，用于 canonical、分享信息、结构化数据和 sitemap。
- `build.mjs`：构建静态输出，只重建本目录下的 `dist/`，不上传仓库其他内容。
- `wrangler.jsonc`：Pages 项目名与构建输出配置。

图标和应用截图来自仓库 `assets/app-icon.png`、`assets/ui-home.png`，版本来自 `Cargo.toml`，无需维护另一套副本。`dist/` 和工具缓存不提交。

网站链接到两个平台的公开发布页，由用户选择安装版 EXE 或绿色版 ZIP；不额外托管安装包、不查询访客账户。网站改版不需要新增 Windows 软件版本或重复发版。
