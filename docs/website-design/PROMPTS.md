# 项目网站设计提示词

工具：内置 ImageGen（未使用 CLI/API 回退）。用途：待确认的项目网站 UI 设计稿。

## 首屏与应用预览

```text
Use case: compositing / ui-mockup. Create the FIRST of three coordinated, high-fidelity desktop website section concept screenshots for 雷神守护 / Leigod Guard. This is a design for user approval, not code. Composite the provided real native Windows app screenshot and existing app icon into a polished product website. Input 1 is the real product screenshot to preserve as a screenshot, including its recognizable sidebar, startup waiting ring, account balance and game list. Input 2 is the existing original app icon; preserve its cyan shield, pause bars, blue square identity. Do not redesign the app or invent product UI. All screenshot values are DEMONSTRATION DATA, not real user data.
Output exactly ONE front-facing website screenshot, 1440 px wide and about 1200 px tall, full section from nav to product screenshot caption. No device frame, no browser chrome, no design board, no annotations outside page.
Art direction: refined Apple-like product editorial page, true white base #FFFFFF, cool pale blue and seafoam Gaussian-blurred backdrop confined behind product, dark navy #172131 type, bright blue #007AFF CTAs, cyan accents. Large disciplined Chinese sans-serif typography, generous white space, crisp text, restrained frosted translucent panels and hairline white borders. No neon gaming wallpaper, no floating orbs, no fake metrics, no eyebrow/pill/badge above headline. This is a WINDOWS app with an Apple-inspired visual style, never macOS-only or Apple official branding.
Layout: quiet 72px white translucent navigation, existing icon about 34px and brand '雷神守护' left, links '功能' '暂停规则' '下载' right. Hero CENTER aligned. At y145 large two-line headline exactly '让加速时长，' then '留给真正开玩的时刻。' (dark first line, tasteful solid bright blue second line). Below two-line subcopy exactly '配合雷神加速器使用的 Windows 开源计时守护工具。' and '游戏退出后按规则暂停，重启后也能检查闲置计时。' Below two aligned 48px high buttons, filled blue primary 'Gitee 国内下载' and white frosted secondary 'GitHub 下载'. Small plain support line below: 'Windows 10 / 11 x64 · 安装版与绿色版 · MIT 开源'. This text belongs below the buttons, not an eyebrow.
Below, centered straight-on actual app screenshot at approximately 1000px wide, preserving its aspect ratio, with 20px radius, fine outline, very soft layered shadow, floating just above the pale blue-green background. No added macOS traffic lights. Use the complete reference screenshot without inventing accounts or remote controls. The small caption below reads '真实应用界面 · 图中时长与游戏名单为演示数据'.
Readable web-native text and controls will be implemented in HTML/CSS later; this is a visual spec. Product screenshot remains a separable asset. Do not add pricing, testimonials, users count, download counts, account login, cloud dashboard, unimplemented remote control or guarantees of savings. Site is an independent open-source project's home, not the official Leigod accelerator company. The next coordinated images will show rules/features, then downloads/FAQ. This image covers the full hero section only, with confident composition and no cropping of the app screenshot.
```

## 规则与功能

```text
Use case: compositing / ui-mockup. Create section TWO of the same 雷神守护 / Leigod Guard single-page project website. First input image is the approved-direction hero concept: use ONLY as style reference, faithfully match its white base, navy Chinese sans typography, blue #007AFF action color, pale blue/seafoam/lavender frosted glass, generous whitespace, quiet fine borders. Second input is real product UI for balance styling reference. Produce a NEW large readable standalone MIDDLE SECTION concept, not a crop of the prior image. User is reviewing designs before website implementation.
Output one front-facing flat website section screenshot, approximately 1440 wide by 1440 high, generous 120px side gutters. No browser/device chrome, no nav/header repetition, no design board. All text must be clean, legible simplified Chinese. No decorative hero eyebrows, pills, fake stats, testimonials, cloud functions, pricing, game artworks or invented product guarantees.
Top: left-aligned strong heading '三种场景，清楚的暂停规则。' and smaller gray line '默认等待可调整；自动暂停需要有效名单、已开启的策略、登录与网络。'
Use THREE open horizontal editorial rows separated by fine lines, not three boxed cards and not a connected chronological timeline. Each has a modest blue number 01 / 02 / 03 left, title plus clear short supporting text center, and large blue timing on the right:
01 title '游戏结束，留一点缓冲' body '先检测到名单游戏运行，全部退出后再等待；重开会取消倒计时。' right '90 秒' small '默认退出宽限期'.
02 title '重启之后，也许只是工作' body '本次启动检查有效、等待后仍无名单游戏时，尝试暂停计时。' right '3 分钟' small '默认启动等待'.
03 title '准备开玩，给自己多一点时间' body '启动检查尚未结束时可延后；检测到游戏后转入退出监控。' right '10 分钟' small '从点击延后时计算'.
After generous spacing, a different split layout: on left a single elegant frosted pale-blue account balance preview tile with words '账户剩余时长', large '128 小时 42 分钟', green small '已暂停', bottom '最近查询 21:47 · 刷新时长', and visible small caption '演示数据'; on right heading '剩余时长，打开就看得到。' with three plain lines, no mini card grid: '启动应用时自动查询' / '从托盘打开或进入账户页时刷新' / '首页与账户页在前台时，每分钟刷新'. Below small note '显示最近查询结果，不模拟逐秒扣费。'
Finally an open, low-density text strip with heading '常用游戏预设，也保留你的自定义。' and one readable line 'PUBG · Counter-Strike 2 · Apex Legends · 英雄联盟'. Supporting small line '可从运行进程选择，或手动填写；预设需按本机实际进程核对。' Final text link '查看完整使用规则 →'.
The website must remain a realistic implementable static single-page HTML/CSS design. Use the same typography and surface family as the hero input. Intentional rhythm: rules are open rows, balance alone is a frosted card, game examples are simple type. Do not make all areas cards, do not add a logo wall or claim verified compatibility. No private real account values. Output exactly one coordinated middle-section image.
```

## 下载与 FAQ 初稿

```text
Use case: compositing / ui-mockup. Produce the THIRD coordinated concept section of the 雷神守护 / Leigod Guard website: download area, three FAQ rows, footer. Use the attached two prior concept screenshots as STYLE REFERENCES ONLY. Create a new standalone full LOWER SECTION, not a crop and do not repeat hero or rules. Preserve the same true-white base, pale cool blue-green Gaussian-blurred accent behind download panel, navy Chinese sans-serif text, #007AFF actions, frosted white surfaces, subtle 20px rounded edges, delicate outlines, generous 100px horizontal gutters. This is an independent open-source Windows utility's project website. The user must approve images before any website code is implemented.
Output ONE flat front-facing desktop website screenshot around 1440px wide x 1500px high. Clean readable Chinese; exact provided copy. No browser/device chrome, no design-board labels, no fake download statistics, app stores, macOS download, subscriptions or signup.
Top headline '现在，让守护留在后台。' large, centered. Subtitle 'Windows 10 / 11 x64 · 安装版与绿色版'.
A single quiet frosted download area, two purposeful side-by-side provider columns, not an unrelated feature grid:
Left title 'Gitee 国内下载', description '国内下载入口，包含安装版、绿色版和校验文件。', primary blue CTA '前往 Gitee 发布页 ↗'.
Right title 'GitHub 下载', description '项目主仓库，查看源码、版本记录与发布文件。', white outlined CTA '前往 GitHub 发布页 ↗'.
Below providers two compact edition explanation rows with fine separators: bold '安装版 EXE' then '双击安装，适合日常使用。'; bold '绿色版 ZIP' then '完整解压后运行，无需安装向导。'. Then small line '已有用户：关于与更新 → 检查更新 → 下载并自动更新'.
Next open white section, heading '下载前，你可能想知道。'. Three FAQ items, all answers visibly expanded, laid out as generous left aligned typographic rows with hairline separators, no accordion chevrons:
Question '这是雷神加速器官方软件吗？'
Answer '不是。这是独立开源项目，需要配合雷神官方客户端使用。'
Question '暂停计时，会停止加速吗？'
Answer '本工具只请求暂停账户计时，不停止客户端中的加速。请到雷神官方微信小程序下拉刷新，核对计时状态。'
Question '突然断电，也能自动暂停吗？'
Answer '断电后本地工具无法工作。下次启动可在等待结束、仍无名单游戏时尝试暂停，需要有效登录和网络；无法追回已消耗的时长。'
Footer after breathing space: small brand '雷神守护 · Leigod Guard', links '使用说明' '隐私说明' '问题反馈' 'MIT License'. Small gray line '独立开源项目，与雷神加速器运营方无隶属、合作或授权关系。'
Faithfully continue visual family and spacing of input references. Ensure all text fits and is legible, no cropped bottom/footer. The eventual site will use code-native text and links, actual release-page URLs, no simulated account queries on the website. No invented claims or guarantees. Exactly one polished lower-section image.
```

## 下载区定向修正

```text
Use case: precise-object-edit / ui-mockup. Make TWO narrow corrections to this existing lower-section website concept for 雷神守护; preserve the entire page layout, white/pale blue/seafoam colors, typography, spacing, glass surfaces, blue buttons, all other content and footer. First: remove BOTH icons next to the provider titles 'Gitee 国内下载' and 'GitHub 下载', leaving simple text-only provider headings, aligned consistently. The app shield is the product icon, not Gitee's provider logo. Keep the app shield in the FOOTER unchanged. Second: correct the last FAQ answer so the exact full text reads '断电后本地工具无法工作。下次启动可在等待结束、仍无名单游戏运行时尝试暂停，需要有效登录和网络；无法追回已消耗的时长。' The word '名单' is essential: the app only checks configured game processes, not every running game. Fit this answer cleanly in two or three lines without cropping. Everything else must stay the same. Output exactly one corrected full lower-section screenshot at the same dimensions, no annotations.
```
