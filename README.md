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

### 1. 准备目录与配置文件

```bash
mkdir -p rx-emby && cd rx-emby

# 下载编排文件与配置模板
curl -fsSL https://raw.githubusercontent.com/Clowrain/rx-emby/main/deploy/docker-compose.yml -o docker-compose.yml
curl -fsSL https://raw.githubusercontent.com/Clowrain/rx-emby/main/deploy/config.example.toml -o config.toml
```

> **提示**：请务必先下载或创建 `config.toml` 再执行启动命令，避免 Docker 将其自动创建为空目录导致启动失败。

### 2. 修改配置

编辑 `config.toml`，重点检查以下配置项（其余参数可保持默认，也可在启动后前往 Web 后台「系统设置」修改并立即生效）：

| 配置项 | 说明 |
| :--- | :--- |
| `[sync].strm_txt_source` | `strm.txt` 索引文件地址（支持本地路径或 `http(s)://` 链接） |
| `[alist].base_url` / `username` / `password` | 小雅或 AList 服务地址及登录凭据（如有两步验证可填写 `otp_secret`） |
| `[alist].path_prefix` | 若小雅挂载在 AList 子目录下（如 `/xiaoya`），请填写该前缀；根目录留空 |
| `[tmdb].api_key` | TMDB v3 API Key（用于获取海报、简介、评分与分集信息） |
| `[tmdb].proxy_url` | 访问 TMDB 所需的 HTTP / SOCKS5 代理地址（直连可留空） |
| `[scraper].auto_background` | 默认 `false`（打开详情或手动触发时按需刮削）；设为 `true` 则在后台持续刮削全库 |

> **注意**：在 Docker Compose 网络中，`[server].database_url` 需使用数据库服务名 `postgres`（`deploy/config.example.toml` 已默认配置）。

### 3. 启动服务

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
2. 确认项目根目录存在 `config.toml`（可从 `deploy/config.example.toml` 复制修改；本地开发时 `[server].database_url` 的主机名应为 `127.0.0.1:5432`）。
3. 编译前端并将静态产物内嵌至 Rust 服务端启动：
   ```bash
   ./run.sh
   ```
   若仅需编译不启动，可执行 `./run.sh --build-only`。
