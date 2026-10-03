# 服务端部署入口

后台 0.7.0 使用 **MySQL 8.4**，不再直接连接 PostgreSQL。0.6.x 及更早的数据库必须先离线迁移；不能直接替换数据库 URL 升级。

完整的 Docker、Linux 可执行文件、邮件、TLS、备份恢复及旧库迁移步骤统一维护在：

**[服务端 MySQL 部署与迁移指南](https://github.com/CMMUU/project-docs/blob/main/leigod-guard/服务端MySQL部署与迁移指南-2026-10-03.md)**

新建 Docker 部署的最短路径（Linux root，空目录）：

```bash
git clone https://github.com/CMMUU/leigod-guard.git
cd leigod-guard/deploy/server
python3 init-config.py --docker
docker compose --env-file server.env up -d --build --wait app
curl --fail http://127.0.0.1:3088/api/health
```

首次初始化只生成本机配置，不会开启远程暂停。公网使用前配置自己的 HTTPS、管理员和邮件账号，并阅读完整指南。不要对已有实例重新运行初始化脚本或重新生成密钥。

Windows 客户端的维护者平台地址为 `https://leigod-login.cmmuu.com`。自建实例需要自行修改 `src/platform_api.rs` 的固定地址并构建客户端；维护者账号和设备不会自动迁入自建实例。
