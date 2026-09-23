# 服务端自行部署：Docker 与 Linux 可执行文件

服务端源码已公开在 [GitHub server/](https://github.com/CMMUU/leigod-guard/tree/main/server)，部署配置在 [deploy/server/](https://github.com/CMMUU/leigod-guard/tree/main/deploy/server)。本文适用于含自部署支持的后台 0.3.2；上海现有部署无需迁移，15 秒心跳、120 秒失联规则不变。

## 先选择运行方式

| 方式 | 包含什么 | 适合谁 |
| --- | --- | --- |
| Docker Compose，推荐新部署 | 本地构建应用镜像、PostgreSQL 18、可选 Caddy HTTPS | 希望按一组命令安装、升级和查看日志 |
| Linux 可执行文件 | `leigod-guard-server` + `static/`；数据库和 HTTPS 单独提供 | 已有 PostgreSQL、Nginx / 1Panel，希望用 systemd 管理 |

服务端必须连接 **PostgreSQL**；一个 EXE/ELF 文件不能替代数据库。无需 Redis 或 Node.js 运行时；已提交的网页静态资源无需自行编译。推荐先在 Linux x86_64 部署；CI 提供此架构的 musl 可执行文件，Docker 冒烟测试也以 Linux x86_64 为准，其他架构需自行构建验证。

以下安装命令使用 **Linux Bash**；创建系统目录、修改密钥所有权和安装 systemd 服务的步骤按 root 执行。Docker Engine 与 Compose 插件按 [Docker 官方安装说明](https://docs.docker.com/engine/install/ubuntu/) 安装。准备 Git、Python 3 和 curl。首次从源码编译需要联网和较多临时内存/磁盘；小服务器可在构建机生成镜像或下载 CI 成品。

**客户端连接限制：** 现有 Windows 正式版 v0.15.1 固定连接维护者的上海后台。自建后台可直接通过网页使用，但现有客户端不会自动改连你的服务器；见文末“Windows 客户端连接自建后台”。部署自己的服务无需、也不应复制维护者的数据库、密钥或 Resend 账号。

## 方式一：Docker Compose

### 1. 下载源码，生成一次性配置

```bash
git clone https://github.com/CMMUU/leigod-guard.git
cd leigod-guard/deploy/server
python3 init-config.py --docker
```

脚本创建 `server.env`（0600）和 `secrets/remote.key`（32 个随机二进制字节、0600、UID 10001），密码均随机生成，不打印秘密。重复执行会拒绝覆盖，升级时不要重新生成。密钥绑定到容器中的 `/run/secrets/remote.key`；容器应用以 UID 10001 运行，不能把密钥改成 0644 来规避权限错误。Linux rootless Docker/用户命名空间的 UID 映射需另行适配，本说明使用普通 rootful Docker。

默认 `PUBLIC_ORIGIN=http://127.0.0.1:3088`、`REMOTE_EXECUTION=false`，可先做本机验收。数据库不向宿主机发布端口，应用只映射宿主机回环 `127.0.0.1:3088`，不会直接开放明文公网 API。

### 2. 启动数据库和后台

```bash
docker compose --env-file server.env up -d --build --wait app
docker compose --env-file server.env ps
curl --fail http://127.0.0.1:3088/api/health
```

健康响应包含 `status: "ok"`、`version: "0.3.2"`、`remote_execution: false`。Compose 等数据库通过健康检查才启动应用；应用启动时自动执行内嵌迁移并提供网页。这里从公开源码本地构建镜像，没有假定已存在公开的 GHCR/Docker Hub 成品镜像。

服务器上没有浏览器时，在自己的电脑执行以下命令，再用浏览器打开 `http://127.0.0.1:3088`：

```bash
ssh -L 3088:127.0.0.1:3088 your-user@your-server
```

### 3. 创建第一个管理员

仍在 `deploy/server`，通过交互输入避免将管理员密码写入命令历史：

```bash
read -r -p '管理员用户名: ' ADMIN_USERNAME
read -r -s -p '管理员密码（12～128 字节）: ' ADMIN_PASSWORD
printf '\n'
export ADMIN_USERNAME ADMIN_PASSWORD
docker compose --env-file server.env run --rm -e ADMIN_USERNAME -e ADMIN_PASSWORD app create-admin
unset ADMIN_USERNAME ADMIN_PASSWORD
```

用户名为 3～100 个 ASCII 字母、数字或 `@._-+`，会转为小写。成功输出 `Administrator created.`，随后在网页登录页使用账号密码登录。**此命令创建账号，不是重置密码；同名账号不可重复创建。** 未配置邮件时，邮箱验证码入口不可用，但管理员密码登录可用。

### 4. 开启公网 HTTPS

编辑 `server.env`，把 `PUBLIC_ORIGIN` 改成你的实际外部地址，例如 `https://guard.example.com`，无末尾斜杠、无路径。它必须与浏览器地址的协议、主机、端口完全一致，否则登录写请求会被拒绝。

**空白服务器、有域名：** 将域名 A/AAAA 记录正确指向服务器，放通 80/443，确保这两个端口没有其他服务占用，然后运行：

```bash
docker compose --env-file server.env --profile https up -d --build --wait
curl --fail https://guard.example.com/api/health
```

可选的 Caddy 服务自动申请和续期域名证书，证书数据使用持久卷。签发依赖 DNS、端口可达及 CA 服务，不能把容器启动当作签发成功；以外部 HTTPS 请求和浏览器证书检查为准。[Caddy 自动 HTTPS 要求](https://caddyserver.com/docs/automatic-https)

**已有 Nginx / 1Panel：** 不启用 `https` profile；在现有入口新增站点，将 HTTPS 请求反代至宿主机 `http://127.0.0.1:3088`。提供了独立的 `nginx-self-host.conf.example`，替换域名和证书路径后，执行 `nginx -t` 再重载。若 Nginx 自身也在隔离容器中，`127.0.0.1` 指向那个容器，需使用已有的宿主网络或明确配置可达的应用网络，不能照抄回环地址。

**只用 IP：** `PUBLIC_ORIGIN=https://你的公网IP`，入口需安装覆盖该 IP、客户端信任且能自动续期的证书。不要假定 Caddy 的域名自动签发流程等价于公网 IP 证书部署，也不要在客户端关闭证书校验。除 `http://127.0.0.1:端口` 本机测试外，后台要求 HTTPS。

### 5. 配置自己的验证码邮件

在自己的 Resend 账号验证发信域名，按控制台要求添加 DNS 记录并取得 API Key，然后取消 `server.env` 中对应注释并填写：

```dotenv
RESEND_API_KEY=re_你的密钥
EMAIL_FROM='雷神守护 <login@你的已验证域名>'
```

`EMAIL_CODE_SECRET` 已由初始化脚本生成，不要重置。未准备邮件服务时，应完全省略 `RESEND_API_KEY` 和 `EMAIL_FROM`，不要填写空字符串。程序不自动读取 Resend 账号配置，也不提供共享发信额度。配置变更后用 `up -d` 重建应用容器，单纯 `restart` 不会更新容器环境：

```bash
docker compose --env-file server.env up -d --wait app
```

如果之前启用了 `https` profile，保留该服务运行；只修改邮件参数不需要重签证书。用自己的收件邮箱测试验证码。邮件域名设置参见 [Resend 官方域名说明](https://resend.com/docs/dashboard/domains/introduction)。

### 6. 开启服务器远程暂停

先完成备份与外部 HTTPS 验证，再把 `REMOTE_EXECUTION=false` 改为 `true`，运行上一节的 `up -d --wait app`。检查 `/api/health` 返回 `remote_execution: true`。

这只开启服务器能力。设备还需在连接此后台的客户端中登录、绑定、明确授权上传雷神凭据，收到当前授权版本的首次心跳后才生效。不能仅凭健康接口就宣称某个账号已被保护。密钥错误、文件权限不私有或文件不是 32 字节时会拒绝启动。启动/入口恢复后仍有观察期；心跳与暂停策略见 [远程保护说明](REMOTE_PROTECTION.md)。

### 7. 日常管理

```bash
# 状态与日志；不要输出 compose config 的完整结果到公开渠道，其中可能含密码
docker compose --env-file server.env ps
docker compose --env-file server.env logs --tail 100 app
# 关闭应用，保留数据库容器
docker compose --env-file server.env stop app
# 停止整套服务，保留数据卷
docker compose --env-file server.env --profile https down
```

日常操作不要加 `down -v`，它会删除数据库/证书卷。固定部署目录和 Compose 项目名；改名可能创建一套空卷，让原数据看起来“消失”。`server.env` 和 `secrets/` 也必须保留。

## 方式二：Linux 可执行文件

### 1. 获取包含网页的运行包

在 [GitHub Actions → Server CI](https://github.com/CMMUU/leigod-guard/actions/workflows/server.yml) 选择 `main` 上成功的运行，下载 `server-linux-x64` artifact 并解压外层 ZIP。**这是保留 14 天、下载通常需要登录 GitHub 的 CI 构建附件，不是 Windows Release 的资产。** 若附件过期，可自行触发 Server CI，或按后面的源码构建方式生成。

有 GitHub CLI 时也可以：

```bash
gh run list --repo CMMUU/leigod-guard --workflow server.yml --branch main --status success --limit 5
# 把 RUN_ID 替换为上一步选定的成功运行编号
gh run download RUN_ID --repo CMMUU/leigod-guard --name server-linux-x64 --dir guard-package
cd guard-package
sha256sum -c server-linux-x64.tar.gz.sha256
mkdir runtime
tar -xzf server-linux-x64.tar.gz -C runtime
cd runtime
```

目录至少包含 `leigod-guard-server` 和 `static/`；含本部署支持的新构建还携带 `docs/` 与 `deploy/server/`。不要只复制二进制而遗漏网页目录。运行机需安装系统 CA 证书，例如 Debian/Ubuntu 的 `ca-certificates`；不需要 Rust 编译器运行已构建的 musl 成品。

### 2. 准备 PostgreSQL 和私有配置

安装 PostgreSQL（已有数据库可以复用，但为此项目新建独立数据库/用户），然后初始化配置：

```bash
# Debian / Ubuntu 的本机数据库示例；已使用现有数据库时跳过安装
apt-get update
apt-get install -y postgresql ca-certificates python3 curl
systemctl enable --now postgresql
python3 deploy/server/init-config.py --output .
sudo -u postgres createuser --pwprompt guard
sudo -u postgres createdb --owner=guard leigod_guard
```

创建 `guard` 时，密码应与生成的 `server.env` 中 `POSTGRES_PASSWORD` 一致。原样使用随机十六进制密码可避免 URI 转义问题；自选密码若含特殊字符，`DATABASE_URL` 中需百分号编码。已有数据库则编辑 `DATABASE_URL`，确保指定用户拥有项目库、能执行迁移；这里不需要 PostgreSQL 全局超级用户。保持数据库仅本机或可信网络可达。

### 3. 初始化管理员并前台运行

在 `runtime` 目录，先按前文填写 `PUBLIC_ORIGIN` 和可选邮件配置。程序**不会自动加载 `.env` 或 `server.env`**；手动运行必须显式导出：

```bash
set -a
. ./server.env
set +a
read -r -p '管理员用户名: ' ADMIN_USERNAME
read -r -s -p '管理员密码（12～128 字节）: ' ADMIN_PASSWORD
printf '\n'
export ADMIN_USERNAME ADMIN_PASSWORD
./leigod-guard-server create-admin
unset ADMIN_USERNAME ADMIN_PASSWORD
./leigod-guard-server
```

这个终端会持续显示服务日志。另开终端执行 `curl --fail http://127.0.0.1:3088/api/health`，本机测试可用前述 SSH 转发访问网页。`Ctrl+C` 停止前台进程；关闭终端不会替你保活，长期运行使用下一节的 systemd。

`LISTEN_ADDRESS` 默认 `127.0.0.1`，保持回环监听再经 HTTPS 反代。Docker 镜像显式改为容器内 `0.0.0.0`，宿主端口仍仅回环开放。`STATIC_DIR` 默认相对当前工作目录的 `static`；可改成绝对路径。

### 4. 安装 systemd 服务

以下是**首次安装**，不是覆盖已有部署的升级脚本。在前台服务停止后执行：

```bash
install -d -m 755 /opt/leigod-guard/current
install -d -m 700 /opt/leigod-guard/secrets
install -m 755 leigod-guard-server /opt/leigod-guard/current/
cp -a static /opt/leigod-guard/current/
install -m 600 server.env /opt/leigod-guard/secrets/server.env
install -m 600 secrets/remote.key /opt/leigod-guard/secrets/remote.key
install -m 644 deploy/server/leigod-guard.service /etc/systemd/system/leigod-guard.service
systemctl daemon-reload
systemctl enable --now leigod-guard
systemctl status leigod-guard --no-pager
journalctl -u leigod-guard -n 100 --no-pager
```

unit 使用 `DynamicUser`、只读系统目录和 `LoadCredential` 交付密钥，需要支持这些功能的 systemd（建议 Debian 12 / Ubuntu 24.04 或更新）。`WorkingDirectory=/opt/leigod-guard/current`，环境文件及凭据从 `/opt/leigod-guard/secrets` 加载。即使关闭远程执行也需保留初始化密钥文件，因为 unit 会加载该文件。

公网入口沿用前文 Nginx/1Panel HTTPS 方式，将模板中的域名与证书路径改为自己实际配置。要开启远程执行，修改**已安装的** `/opt/leigod-guard/secrets/server.env`，设置 `REMOTE_EXECUTION=true`，再 `systemctl restart leigod-guard`，不是修改原下载目录中的副本。数据库外置时保持 `DATABASE_URL` 可达。

### 5. 不下载 CI 成品，自己编译

在 Linux 构建机安装 [Rust 官方工具链](https://www.rust-lang.org/tools/install)。Debian/Ubuntu 安装 `build-essential` 和 `musl-tools`，然后在源码仓库执行：

```bash
rustup target add x86_64-unknown-linux-musl
cd server
cargo build --locked --release --target x86_64-unknown-linux-musl
mkdir -p bundle
cp target/x86_64-unknown-linux-musl/release/leigod-guard-server bundle/
cp -a static bundle/
mkdir -p bundle/deploy bundle/docs
cp -a ../deploy/server bundle/deploy/
cp ../docs/SERVER_DEPLOYMENT.md ../docs/REMOTE_PROTECTION.md ../docs/REMOTE_GUARD_PLAN.md bundle/docs/
```

将 `bundle` 作为上述 `runtime` 目录使用。使用干净源码构建；不要把自己部署时生成的 `server.env`、`secrets/` 打包分发。数据库迁移已编译内嵌，无需在运行机再安装 SQLx CLI。ARM64 不能直接运行这份 x86_64 文件，需针对目标架构另行构建验证。

## 备份、恢复、升级

Docker 的 PostgreSQL 18 持久卷挂在 `/var/lib/postgresql`，与 PostgreSQL 17 及更早镜像的目录约定不同，见 [官方镜像说明](https://hub.docker.com/_/postgres)。**数据卷不是备份。** 至少定期备份数据库、`server.env`、`remote.key`，并把加密副本保存到另一台机器；丢失远程密钥后，数据库里的雷神凭据密文无法解密。

Compose 部署目录的数据库备份示例：

```bash
umask 077
mkdir -p backups
guard_backup="backups/guard-$(date -u +%Y%m%dT%H%M%SZ).dump"
docker compose --env-file server.env exec -T postgres pg_dump -U guard -d leigod_guard -Fc --no-owner --no-acl > "$guard_backup"
docker compose --env-file server.env exec -T postgres pg_restore --list < "$guard_backup" > /dev/null
```

只有导出和清单检查均成功才把文件当成可用备份。环境文件、密钥和 Caddy 证书数据另行以私密方式备份；不要提交到 Git。可执行文件方式同样用 `pg_dump` 导出指定库，使用受限 `.pgpass` 等方式提供密码，避免将密码写进命令参数。

恢复演练应创建一个空的独立数据库，用 `pg_restore --no-owner --no-acl` 导入，并以 `REMOTE_EXECUTION=false` 启动隔离实例。核对管理员/设备记录及密钥匹配后再制定生产切换步骤，不直接覆盖在用库。仓库原有 `backup.sh` 和异地备份脚本针对维护者的上海/HK 路径、容器名与 SSH 环境，不能直接作为通用安装脚本执行。

升级时先备份，再获取固定的已验证源码提交或 CI 产物。Compose 复用原配置/项目名/卷，执行 `up -d --build --wait app`；可执行方式放入新的 release 目录，切换 `current` 后重启 systemd。每次检查本地及公开 `/api/health`、网页登录、静态资源与日志。应用会自动迁移数据库，旧程序不一定兼容新 schema；不要为回滚删除 migration 记录或用旧备份覆盖新数据。异常时可先关闭 `REMOTE_EXECUTION` 并重启，保留数据后向前修复。

## Windows 客户端连接自建后台

当前没有“自定义服务器地址”的设置项，也没有可直接覆盖它的运行时环境变量。要连接自己的后台：

1. 在自己的 Windows 源码副本中，把 `src/platform_api.rs` 的 `ORIGIN_URL` 常量改为自建后台的准确 HTTPS origin，无尾部斜杠；不要关闭证书校验。
2. 按仓库 README 的 Windows MSVC 构建步骤构建自己的客户端。不要把此改动提交到上游来替换所有用户的默认服务。
3. 自建客户端重新登录自己的平台账号并注册/配对设备；维护者上海平台的账号、会话和设备授权不会自动迁移。开启远程保护时重新明确授权。
4. 官方客户端更新可能覆盖你的地址修改。自建客户端用户应关闭官方自动检查，不应用官方二进制，或维护自己的发行流程；目前没有独立的自部署更新通道。

后台网页可独立运行；“网页能登录”不代表现有官方客户端已连接自建后台。

## 常见问题与验收口径

| 现象 | 检查 |
| --- | --- |
| 页面 404 / 样式缺失 | `static/` 是否完整；工作目录及 `STATIC_DIR` 是否正确 |
| 登录报来源不匹配 | 浏览器地址是否与 `PUBLIC_ORIGIN` 完全一致，是否多了斜杠或用了不同端口 |
| Docker 显示健康但外网访问不到 | 宿主 3088 刻意仅回环开放；检查 HTTPS 入口、DNS、证书和反代网络 |
| 验证码不可用 | 三个邮件变量、Resend 域名验证、密钥权限和服务端出站网络 |
| 密钥权限或长度错误 | 原始 32 字节、0600；Docker 所有者为 UID 10001，systemd 用 LoadCredential |
| 重建后密码无效 | 改 `POSTGRES_PASSWORD` 不会自动修改已有数据库用户密码；保留原配置或显式进行数据库密码轮换 |
| 能登录但不会远程暂停 | 服务器执行开关、设备授权、首次心跳、凭据有效性、观察期及服务异常状态 |

CI 验证范围包括：构建 Dockerfile、真实 Compose 网络、PostgreSQL 迁移、静态网页、非 root 应用、私有密钥挂载、管理员登录、容器重建后的数据保留、数据库导出/隔离恢复，以及打包后的 musl 可执行文件启动。CI 使用一次性数据，没有验证你的域名证书签发、邮箱收信或真实雷神暂停；这些需要在你自己的环境验收。

## 0.5.0：云端网吧模式

Docker 与 Linux 可执行文件的部署入口保持不变，无需新增环境变量。保留现有数据库和 `REMOTE_KEY_FILE` 密钥，备份后升级；应用自动执行 `0005_cafe_mode.sql`。迁移后不要直接回退到未包含迁移的旧版程序。

用户登录后台 → 网吧模式 → 选择已在客户端授权的加速器账号 → 设置网吧模式。默认时长为 24 小时，可设置 1–168 整数小时，必须用户本人开启。网页可以关闭，计时与执行由服务端持久化；部署不会自动开启真实账号。完整计时、授权和故障边界见 [远程保护说明](REMOTE_PROTECTION.md#网吧模式后台-050)。

## 0.4.0：两款加速器的失联保护

后台 0.4.0 配合客户端 0.17.0，分别支持雷神和外星仔授权。同一设备可以拥有两个独立保护开关；既有雷神客户端与接口继续兼容。公网放行仅需原 HTTPS 入口，服务器出站需要能访问 `webapi.leigod.com` 和 `api.et-api.com` 的 HTTPS 服务。

升级前按本文备份数据库与应用密钥；新二进制启动时应用 `0004_provider_grants.sql`。迁移增加 provider 字段与独立授权版本，不覆盖已有雷神密文、用户或设备。执行迁移后不能直接换回不认识迁移的旧 0.3.x 程序；故障时先将 `REMOTE_EXECUTION=false` 并使用包含全部迁移的新程序启动，再向前修复。不要删迁移记录或覆盖新增用户数据。

不需要新增密钥、Redis 或数据库。Docker 与可执行文件使用同一迁移与环境变量。外星仔由用户在客户端单独上传令牌、设备 ID 和校准值；后台不接收密码。校准/账号身份不符合官方查询时拒绝授权。首次上线及重启仍观察 120 秒；真实暂停验收必须安排在空闲账号上。
