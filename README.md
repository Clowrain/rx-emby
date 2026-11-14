# RX-Emby

基于 Rust（Axum + SQLx + PostgreSQL）与 Vue 3 构建的轻量级 Emby 协议媒体服务器。面向小雅 / AList 网盘资源，提供零落盘索引入库、302 直链转发、TMDB 元数据刮削、本地图片缓存与内置 Web 管理后台。

## 核心特性

- **零落盘媒体库同步**：直接解析 `strm.txt` 索引文件增量入库，自动清理失效片源，无需在本地挂载或生成大量 `.strm` 实体文件。
- **302 网盘直链转发**：自动对接 AList / 小雅获取播放直链并内存缓存复用；单集播放至 85% 时自动预热下一集直链，减少切集等待。
- **跨目录聚合并去重**：同部影视在不同网盘目录的多版本片源自动合并展示，按画质标签优先级排序。
- **TMDB 刮削与海报墙**：支持按需刮削或后台自动刮削，提供本地海报与剧照磁盘缓存（支持容量上限与自动清理）。
- **智能视图（Smart Views）**：按地区、播出平台、流派、评分与发行年份组合筛选规则，在客户端首页生成专属分类栏目。
- **多用户权限隔离**：支持为家庭成员配置独立账号、密码及可见媒体分类（例如儿童账号仅开放「动漫」「纪录片」），并可管理已登录客户端设备。

---

## Docker Compose 一键部署（推荐）

镜像由 GitHub Actions 自动构建并发布至 GHCR（支持 `linux/amd64` 与 `linux/arm64`）：

```text
ghcr.io/clowrain/rx-emby:latest
```

新版采用 **`.env` 单源配置**，无需再手动挂载 `config.toml`。

### 1. 准备目录与配置文件

```bash
mkdir -p rx-emby && cd rx-emby

# 下载编排文件与环境变量模板
curl -fsSL https://raw.githubusercontent.com/Clowrain/rx-emby/main/deploy/docker-compose.yml -o docker-compose.yml
curl -fsSL https://raw.githubusercontent.com/Clowrain/rx-emby/main/deploy/.env.example -o .env
```

### 2. 修改 `.env` 配置

编辑 `.env`，重点检查以下参数（其余参数保持默认即可）：

| 环境变量 | 说明 |
| :--- | :--- |
| `POSTGRES_PASSWORD` | **必填**。PostgreSQL 数据库密码，Compose 会自动将其拼装为 `DATABASE_URL` |
| `XIAOYA_DIR` | 宿主机小雅数据目录（只读挂载到容器 `/xiaoya-db`），用于直接读取本地 `strm.txt` |
| `STRM_TXT_SOURCE` | `strm.txt` 路径或远程 URL（配合 `XIAOYA_DIR` 挂载时默认 `/xiaoya-db/strm.txt`） |
| `ALIST_BASE_URL` / `ALIST_USERNAME` / `ALIST_PASSWORD` | 小雅或 AList 服务地址及登录凭据（如有两步验证可填写 `ALIST_OTP_SECRET`） |
| `ALIST_PATH_PREFIX` | 若小雅挂载在 AList 子目录下（如 `/xiaoya`），填写该前缀；根目录留空 |
| `TMDB_API_KEY` | TMDB v3 API Key（用于获取海报、简介、评分与分集信息） |
| `TMDB_PROXY_URL` | 访问 TMDB 所需的 HTTP / SOCKS5 代理地址（直连可留空） |
| `SCRAPER_AUTO_BACKGROUND` | 默认 `false`（打开详情或手动触发时按需刮削）；设为 `true` 则在后台持续刮削全库 |

> **配置优先级说明**：服务首次启动时会将环境变量中的运行参数写入数据库；之后在 Web 后台「系统设置」修改的内容会持久化在数据库中，优先于 `.env` 生效。

### 3. 确认外部网络并启动服务

`deploy/docker-compose.yml` 默认附加了外部网络 `proxy_net`（用于与宿主机上的代理容器互通）：

- 如果你已有 `proxy_net` 网络，直接启动即可；
- 如果不需要外部代理网络，请注释掉 `docker-compose.yml` 中的 `proxy_net` 相关行（或先执行 `docker network create proxy_net`）。

```bash
docker compose up -d
```

### 4. 访问后台与连接客户端

- **Web 管理后台**：浏览器访问 `http://<你的IP>:8096`，可使用「数据总览」「媒体库海报墙」「智能视图」「用户与活跃会话」「媒体同步与图片缓存」「系统设置」全部功能。
- **Emby 客户端连接**：在 **Infuse**、**VidHub**、**SenPlayer** 等客户端中添加 Emby 服务器：
  - 服务器地址：`http://<你的IP>:8096`
  - 默认管理员账号：`admin`（初始密码为空，留空即可登录；建议随后在 Web 后台「用户与活跃会话」中设置密码或创建家庭成员账号）

---

## 本地构建 Docker 镜像

如需基于本地源码自行构建镜像：

```bash
docker build -t rx-emby:local .
```

或使用 `deploy/docker-compose.yml` 时取消 `build:` 注释并运行：

```bash
docker compose -f deploy/docker-compose.yml up -d --build
```

---

## 本地开发运行

### 前置依赖

- Docker & Docker Compose（用于运行本地 PostgreSQL）
- Rust 工具链（`cargo`）
- Node.js 或 Bun（用于编译 `frontend/` 前端工程）

### 启动步骤

1. 启动本地 PostgreSQL：
   ```bash
   docker compose up -d
   ```
2. 准备本地配置（二选一）：
   - 复制配置模板到根目录：`cp deploy/config.example.toml config.toml`，并将 `database_url` 中的主机名改为 `127.0.0.1:5432`；
   - 或直接通过环境变量传入配置（缺失 `config.toml` 时会自动回退到内置默认值并读取环境变量覆盖）。
3. 编译前端并将静态产物内嵌至 Rust 服务端启动：
   ```bash
   ./run.sh
   ```
   若仅需编译不启动，可执行 `./run.sh --build-only`。
