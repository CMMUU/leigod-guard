# SEO 与 AI 可发现性维护

唯一正式网站为 [leigod.cmmuu.com](https://leigod.cmmuu.com/)。目标是让用户搜索“雷神守护”“雷神加速器自动暂停计时”“游戏退出自动暂停雷神”“雷神剩余时长查询工具”时，有清楚、可核对的项目介绍与下载入口。按真实功能解释常见游戏使用场景，不堆砌游戏名、不生成重复关键词页面。

## 已在源码实现的基础

| 项目 | 实现与用途 |
| --- | --- |
| 可直接读取的正文 | 首页采用静态 HTML，下载、规则、FAQ 无需执行 JavaScript。 |
| 唯一地址 | 首页 canonical 指向正式域名；默认 Pages 域名使用 noindex 响应头，减少重复收录。 |
| 页面身份 | 中文标题、摘要、分享卡片、真实截图说明，以及 WebSite、WebPage、SoftwareApplication 和 FAQPage JSON-LD。 |
| 内容一致 | FAQ 的可见文字、JSON-LD 与 Markdown 来自同一份 JSON；版本、内容修改时间来自 Cargo 与 Git。 |
| 抓取入口 | robots 允许主流搜索及 AI 搜索/用户检索，sitemap 列出唯一首页。不存在的路径返回真正的 404。 |
| AI 可读资料 | `/index.html.md` 是项目概览，`/llms.txt` 是资料索引，`/llms-full.txt` 合并概览与 README 详细规则。 |
| 更新通知 | `Website discovery` 工作流确认生产网站与该次构建一致后向 IndexNow 提交首页。 |

结构化数据描述事实，不编造评分、评论或下载量；添加 FAQPage 也不代表该类网站会获得 Google FAQ 富结果。公开资料强调 Windows 支持范围、独立项目身份、暂停计时与停止加速的区别、断电限制、微信小程序核对方法。

## 搜索发现与模型训练分别管理

Googlebot、Bingbot、Baiduspider，以及 OAI-SearchBot、Claude-SearchBot、PerplexityBot 等搜索爬虫允许抓取。ChatGPT-User、Claude-User、Perplexity-User 是用户触发的检索入口，其访问本身不等于建立搜索索引。

GPTBot、ClaudeBot 等训练爬虫维持禁止抓取的偏好；robots 是对遵守协议的爬虫的指示，不是访问控制。Cloudflare 的托管 robots、WAF 和 AI Crawl Control 还可能影响最终响应，不能只检查仓库文件。排查误拦截时使用官方公布的验证方式和 IP 范围，不给任意伪装 User-Agent 的请求绕过所有安全检查。

`llms.txt` 是可选的开放提案，提供清晰资料索引，**不是 Google、ChatGPT 或其他 AI 的强制收录协议**。Google 的 AI 搜索功能仍依赖正常的搜索抓取、索引与内容质量；没有保证被引用的特殊文件、标签或提交按钮。

## 平台提交与检查

1. **Google**：在 [Search Console](https://search.google.com/search-console) 选择已验证的 `cmmuu.com` 域名资源，提交 `https://leigod.cmmuu.com/sitemap.xml`。用网址检查查看首页是否可抓取、选用的 canonical 和索引状态；必要时请求编入索引。已验证父域名资源可管理其子域名，无需重复添加 DNS 验证记录。
2. **Bing / IndexNow**：自动工作流向 [IndexNow](https://www.indexnow.org/) 发送更新通知，供参与该协议的搜索引擎处理。HTTP 200 表示收到，HTTP 202 表示收到但验证待处理，两者均不是已收录证明。可在 [Bing Webmaster Tools](https://www.bing.com/webmasters/) 验证网站、提交 sitemap 并查看抓取与索引结果。
3. **百度**：登录 [百度搜索资源平台](https://ziyuan.baidu.com/linksubmit/index)，添加并验证 `https://leigod.cmmuu.com`，按平台提供的普通收录入口提交首页或 sitemap。账户权限和配额以控制台实际显示为准；IndexNow 不替代这一步。网站验证文件/标签属于公开证明，可随网站维护；百度提交 API token 如需自动化，应保存到 Actions Secrets，不能放进仓库。

站点验证、通知已接收、爬虫已访问和页面已被索引是四个不同阶段。只有平台对该 URL 的索引报告或实际搜索结果，才能作为已收录的证据；Cloudflare 主域的汇总爬虫请求量不能证明本子站已收录。

## 后续维护节奏

- 发布功能变化时，先更新 README、首页和 `website/faq.json` / `overview.md`；推送会触发网站部署与 IndexNow 通知。不要为未变化的内容反复修改 lastmod 或重复提交。
- 初次提交后约一到两周查看 Google、Bing、百度实际报告。根据“无法抓取”“重复 canonical”“尚未编入索引”等具体原因处理，不承诺收录期限。
- 有真实新需求时再增加对应说明，例如如何核对游戏进程、异常关机的边界、剩余时长刷新排查。内容应帮助使用软件，并从首页及 README 链接过去。
- 从 GitHub / Gitee 项目说明与公开版本说明链接官网；社区介绍应准确披露独立项目身份，遵循社区发布规则。避免购买垃圾外链或批量铺设近似页面。
- 若要衡量实际搜索词、点击量或 AI 引用，优先使用站长平台报告；当前网站未添加访客统计脚本，也未承诺任何 AI 已引用本项目。

## 技术验收

正式首页应返回 HTTP 200，canonical 正确且没有 noindex；未知路径应返回 404。默认 `leigod-guard.pages.dev` 应带 noindex，Markdown 为 `text/markdown`，AI 文本为 UTF-8 `text/plain`，sitemap 为 XML。检查线上最终 robots，确保搜索爬虫未被托管规则覆盖禁止。

工作流失败时检查生产构建状态、内容是否与提交一致、验证文件是否可读取，再手动重跑。不要在旧版仍在线时强行跳过部署验证。实现和本地预览方法见 [网站 README](../website/README.md)。

## 官方参考

- [Google：AI 搜索功能与网站](https://developers.google.com/search/docs/appearance/ai-features)
- [OpenAI：搜索、用户检索与训练爬虫](https://developers.openai.com/api/docs/bots)
- [Anthropic：爬虫分类与控制](https://privacy.anthropic.com/en/articles/8896518-does-anthropic-crawl-data-from-the-web-and-how-can-site-owners-block-the-crawler)
- [Perplexity：爬虫说明](https://docs.perplexity.ai/docs/resources/perplexity-crawlers)
- [llms.txt 开放提案](https://llmstxt.org/)
- [IndexNow 协议](https://www.indexnow.org/documentation)
- [Cloudflare：托管 robots.txt](https://developers.cloudflare.com/bots/additional-configurations/managed-robots-txt/)
- [Cloudflare Pages：响应头](https://developers.cloudflare.com/pages/configuration/headers/)
