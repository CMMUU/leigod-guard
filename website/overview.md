# 雷神守护 Leigod Guard

> 配合雷神加速器使用的独立开源 Windows 计时守护工具：按游戏进程请求暂停账户计时，重启后检查闲置计时，查询账户剩余时长。

项目网站：{{SITE_URL}}/
资料对应仓库版本：v{{APP_VERSION}}
内容最近修改：{{LAST_MODIFIED}}

雷神守护不是雷神加速器官方产品，与其运营方没有隶属、合作或授权关系。软件采用 MIT 许可证，目前提供 Windows 10 / 11 x64 版本。游戏、线路选择和开启加速仍由雷神官方客户端完成。

Leigod Guard is an independent, MIT-licensed Windows companion for the Leigod accelerator. It monitors configured game processes, requests account billing pauses, and displays the latest queried remaining time. It does not stop client acceleration or operate after the computer loses power.

## 下载与更新

- [Gitee 国内发布页](https://gitee.com/cmmuu/leigod-guard/releases)：安装版 EXE、绿色版 ZIP 和校验文件。
- [GitHub 最新正式版](https://github.com/CMMUU/leigod-guard/releases/latest)：主发布源与备用下载入口。
- 安装版：双击 EXE 安装；绿色版：完整解压 ZIP 后运行。
- 已有用户在应用内「关于与更新 → 检查更新 → 下载并自动更新」升级。自动模式比较可用的新版本，同版本优先 Gitee，GitHub 备用；也可明确选择单一来源。
- 两个平台的同步可能有延迟，下载时以实际发布页为准。

## 三种暂停场景

1. 游戏退出：先检测到名单游戏运行，全部退出后默认连续等待 90 秒，再复查并尝试暂停；游戏重新运行会取消倒计时。
2. 本次启动检查：检查有效时，默认连续等待 3 分钟；仍无名单游戏运行则尝试暂停。
3. 准备游戏：尚未结束的启动检查可以延后，保护到至少点击后 10 分钟；不能重启已经结束的启动检查。

这些是独立场景，等待时间不会相加。需要有效游戏名单、已开启的策略、登录与网络。异常断电后工具无法继续工作，也不能追回已经消耗的时长。

## 剩余时长

启动、从托盘打开和进入账户页时自动查询；首页或账户页在前台时每分钟刷新。界面显示最近查询结果与时间，不模拟逐秒扣费。网站中的 128 时 42 分 00 秒和游戏名单都是演示数据，不是访客账户信息。

## 常用游戏

预设示例包括 PUBG（绝地求生）、Counter-Strike 2（CS2）、Apex Legends 和英雄联盟。可从运行进程选择或自定义 `.exe` 名称；预设须按本机实际进程核对，不代表每款游戏、区服均已验证兼容性。

## 常见问题

{{FAQ_MARKDOWN}}

## 来源与维护

- [GitHub 源码](https://github.com/CMMUU/leigod-guard)
- [Gitee 镜像与使用说明](https://gitee.com/cmmuu/leigod-guard)
- [完整使用说明](https://gitee.com/cmmuu/leigod-guard/blob/main/README.md)
- [隐私说明](https://gitee.com/cmmuu/leigod-guard/blob/main/docs/PRIVACY.md)
- [版本记录](https://gitee.com/cmmuu/leigod-guard/blob/main/CHANGELOG.md)
- [问题反馈](https://github.com/CMMUU/leigod-guard/issues)
- [MIT 许可证](https://gitee.com/cmmuu/leigod-guard/blob/main/LICENSE)
- [AI 资料索引]({{SITE_URL}}/llms.txt)
- [资料与详细规则全文]({{SITE_URL}}/llms-full.txt)

本资料由项目仓库构建，为任何访客和检索工具提供相同内容。它描述当前实现，不承诺搜索引擎或 AI 一定收录、推荐或引用本项目。
