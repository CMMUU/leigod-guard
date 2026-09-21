# 雷神守护服务端

Rust/Axum 后台、PostgreSQL 迁移与管理网页均已公开存放在本目录。数据库迁移内嵌在可执行文件中，启动时自动执行；网页资源位于 `static/`，部署时也必须携带。

**[完整自部署说明：Docker Compose 与 Linux 可执行文件](../docs/SERVER_DEPLOYMENT.md)**

- Docker：使用 [Dockerfile](Dockerfile) 和 [Compose 配置](../deploy/server/compose.yaml)，在自己的服务器构建镜像并启动数据库与应用。
- 可执行文件：从 [Server CI](https://github.com/CMMUU/leigod-guard/actions/workflows/server.yml) 下载 `server-linux-x64` 构建附件，或者用 Rust 编译；无需安装 Node.js 运行后台。
- PostgreSQL 是必需依赖；Redis 不是本项目依赖。邮箱验证码需配置自己的 Resend 账号与已验证发信域名。
- Windows 正式版 v0.15.1 固定连接维护者的上海后台。自建后台不会自动替换该地址；自用客户端的地址修改及构建步骤见完整说明。

默认关闭服务器远程执行。此部署支持不改变现有 15 秒心跳、120 秒失联判定。
