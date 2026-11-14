# RX-Emby

基于 Rust (Axum + SQLx + PostgreSQL) 与 Vue 3 构建的轻量级高性能 Emby 协议媒体服务器，支持对接 AList、TMDB 刮削、本地图片 LRU 缓存与 Web 管理后台。

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

### 2. 修改配置

编辑 `config.toml`，至少修改以下配置项：
- `[sync].strm_txt_source`：你的 `strm.txt` 源地址
- `[alist]`：`base_url`、`username`、`password`
- `[tmdb].api_key`：你的 TMDB API Key（可选，也可在后台热配置中修改）

> **注意**：在 Docker Compose 网络中，`[server].database_url` 需使用数据库服务名 `postgres`（`deploy/config.example.toml` 中已默认配置好）。

### 3. 启动服务

```bash
docker compose up -d
```

启动后访问 `http://<你的IP>:8096` 即可进入管理后台，Emby 客户端可直接连接端口 `8096`。

---

## 本地构建 Docker 镜像

如需自行构建镜像：

```bash
docker build -t rx-emby:local .
```

或使用 `deploy/docker-compose.yml` 时取消 `build:` 注释并运行：

```bash
docker compose -f deploy/docker-compose.yml up -d --build
```

---

## 本地开发运行

1. 启动本地 PostgreSQL：
   ```bash
   docker compose up -d
   ```
2. 编译前端并启动 Rust 服务端：
   ```bash
   ./run.sh
   ```
