# RX-Emby 兼容 Emby API 服务实施方案

> 本文档基于 [Emby Developer 官方文档](https://dev.emby.media/doc/index.html) 与 [Emby Server REST API 参考手册](https://dev.emby.media/reference/RestAPI.html) 编写，结合 `rx-emby` 当前代码基线（`src/web/emby_api/`、`src/web/mod.rs`、`Cargo.toml`），制定分阶段、可落地的全量 Emby 协议兼容实施路线。

---

## 一、现状基线与兼容差距评估

`rx-emby` 已在 Axum 中同时挂载 `/emby/*` 与根路径 `/*`（`src/web/emby_api/mod.rs`），跑通了 `VidHub` / `Infuse` 的核心看片链路（登录、视图、列表、详情、`PlaybackInfo`、302 直链播放、基础进度上报）。但距离完整兼容 Emby 官方客户端（Web / iOS / Android / TV / Kodi）及多用户生态，仍有以下核心缺口：

| 模块 | 当前已实现（`src/web/emby_api`） | 主要差距与兼容风险 |
| :--- | :--- | :--- |
| **认证与系统** | `/System/Info/Public`、`/System/Info`（共用 `system_info_public`）、`/Users/AuthenticateByName`、`/Users/:user_id` | 仅解析 `x-emby-authorization` 与 `x-emby-token` / `x-mediabrowser-token`，未兼容标准 `Authorization: Emby ...` 头（[User Authentication](https://dev.emby.media/doc/restapi/User-Authentication.html)）；缺少 `/Users/Public`、`/System/Ping`（`GET/HEAD/POST`）、`/Sessions/Capabilities[/Full]`、`/Sessions/Logout` |
| **媒体库与检索** | `/Users/:user_id/Views`、`/Users/:user_id/Items`、`/Resume`、`/Latest`、`/Root`、`/Items`、`/Search/Hints`、`/Shows/*` | 缺少 `/Items/Counts`、`/Genres`、`/Persons`、`/Studios`、`/Artists`、`/Years`、`/Tags`、`/OfficialRatings`、`/Localization/ParentalRatings` 及 `/Items/:id/Similar` |
| **播放与媒体流** | `/Items/:id/PlaybackInfo`、`/Videos/:id/stream`、`/Videos/:id/:file_name`、`/MediaSegments/:id` | 缺少 `HEAD` 方法支持（部分客户端直连前先发 `HEAD` 探测）；缺少 `/Playback/BitrateTest`、字幕流 `/Videos/:id/:msId/Subtitles/:idx/Stream.:fmt` |
| **会话与实时通道** | `POST /Sessions/Playing[/Progress\|Stopped]`、`PlayedItems`、`FavoriteItems` | 缺少 WebSocket 端点（`/embywebsocket` / `/socket`，`Cargo.toml` 已开启 `axum` `ws` 特性但未挂载）；缺少 `GET /Sessions`、`UserData` 单项接口 |
| **合集与偏好** | 暂无 | 缺少 `PlaylistService`、`CollectionService`、`DisplayPreferencesService`（Web/TV 端保存排序与视图样式必调） |

---

## 二、分阶段实施路线图

### 阶段 P0：协议底座与客户端握手闭环

> 目标：保障所有官方及第三方 Emby 客户端能够顺利完成服务器发现、登录握手、能力上报与基础流媒体探测，不触发 `401` / `405` / `404` 阻断。

1. **统一认证与设备上下文 Extractor（`src/web/emby_api/auth.rs`）**
   - 依据 [User Authentication](https://dev.emby.media/doc/restapi/User-Authentication.html) 与 [API Key Authentication](https://dev.emby.media/doc/restapi/API-Key-Authentication.html) 规范，抽象统一的 `EmbyAuthContext`：
     - **Token 提取优先级**：`X-Emby-Token` → `X-MediaBrowser-Token` → `Authorization: Emby ..., Token="..."` → 查询参数 `api_key`。
     - **设备上下文解析**：同时解析标准 `Authorization: Emby ...` 与 `X-Emby-Authorization`，提取 `UserId`、`Client`、`Device`、`DeviceId`、`Version` 注入请求上下文。
2. **补齐握手与会话核心端点**
   - `GET/HEAD/POST /System/Ping`：返回 `"Emby Server"`（`200 OK`）。
   - 拆分 `/System/Info/Public`（匿名返回 `PublicSystemInfo`）与 `/System/Info`（鉴权返回完整 `SystemInfo`）。
   - `GET /Users/Public`：返回允许在登录页展示的用户列表（含 `HasPassword`、`PrimaryImageTag`）。
   - `POST /Sessions/Capabilities` 与 `POST /Sessions/Capabilities/Full`：接收设备能力声明并写入内存会话表（`DashMap`）。
   - `POST /Sessions/Logout`：吊销当前 `AccessToken` 并清理会话缓存。
3. **补全 `HEAD` 探测与测速桩**
   - 为 `/Videos/:item_id/stream`、`/Videos/:item_id/:file_name`、`/Items/:item_id/Images/:image_type`（及带索引路径）补齐 `HEAD` 路由。
   - 新增 `GET /Playback/BitrateTest`：按 `Size` 参数返回固定字节流，防止客户端因测速失败误判带宽过低而强制请求转码。

### 阶段 P1：完整媒体库导航、字幕与多用户进度

> 目标：支撑官方客户端“类型 / 年份 / 演员 / 制片厂”二级浏览页面、外挂字幕加载与多用户跨端续播。

1. **分类索引与统计接口**
   - 复用 PostgreSQL 现有标签字段（`genres`、`countries`、`networks`、`year_decade`），实现：
     - `TagService`：`GET /Years`、`GET /Tags`、`GET /OfficialRatings`
     - `LocalizationService`：`GET /Localization/ParentalRatings`
     - `GenresService`：`GET /Genres`、`GET /Genres/:name`
     - `PersonsService` / `StudiosService` / `ArtistsService`：`GET /Persons`、`GET /Persons/:id`、`GET /Studios`、`GET /Artists`
     - `LibraryService`：`GET /Items/Counts`、`GET /Items/:id/Similar`
2. **外挂/内封字幕接口（`SubtitleService`）**
   - 实现 `GET/HEAD /Videos/:id/:media_source_id/Subtitles/:index/Stream.:format`（及 `/Items/:id/...` 别名），对接 AList/Xiaoya 同目录外挂字幕（`.srt` / `.ass` / `.vtt`），并在 `PlaybackInfo` 的 `MediaStreams` 中声明真实 `SubtitleStream`（`DeliveryMethod: "External"`）。
3. **用户头像与多用户 `UserData` 闭环**
   - 实现 `GET/HEAD /Users/:id/Images/:image_type`（返回用户头像或默认占位图）。
   - 实现 `POST /Users/:user_id/Items/:item_id/UserData` 与 `POST /Users/:user_id/Items/:item_id/HideFromResume`，将 `PlaybackPositionTicks`、`Played`、`PlayCount`、`LastPlayedDate` 持久化至 PostgreSQL。

### 阶段 P2：WebSocket 实时通道与列表生态

> 目标：打通实时双向同步、自定义播放列表、合集与客户端界面偏好保存。

1. **WebSocket 服务（`/embywebsocket` 与 `/socket`）**
   - 利用 `Cargo.toml` 已启用的 `axum` `ws` 特性构建 `SessionHub`（`DashMap<SessionId, mpsc::Sender<Message>>`）：
     - 握手鉴权：校验 Query 参数 `api_key` 与 `deviceId`。
     - **上行处理**：支持 `ReportPlaybackStart`、`ReportPlaybackProgress`、`ReportPlaybackStopped`（按 [Playback Check-ins](https://dev.emby.media/doc/restapi/Playback-Check-ins.html) `{MessageType, Data}` 结构复用 `sessions.rs` 逻辑）及 `KeepAlive`。
     - **下行推送**：媒体库增量同步完成或用户操作已播/收藏时，推送 `LibraryChanged`、`UserDataChanged`、`Sessions` 更新。
2. **合集、播放列表与界面偏好**
   - `DisplayPreferencesService`：`GET/POST /DisplayPreferences/:display_preferences_id`（按 `UserId + Client` 持久化视图排序、卡片比例等设置）。
   - `PlaylistService`：`POST /Playlists`、`GET/POST/DELETE /Playlists/:id/Items`、`POST /Playlists/:id/Items/:item_id/Move/:new_index`。
   - `CollectionService`：`POST /Collections`、`POST/DELETE /Collections/:id/Items`。
   - `SuggestionsService` / `MoviesService`：`GET /Users/:user_id/Suggestions`、`GET /Movies/Recommendations`。

### 阶段 P3：后台运维对齐与“非目标能力”优雅降级

> 目标：针对 `rx-emby` 基于 Xiaoya STRM + AList + 115 CDN 302 直链的架构定位，对重型转码/广播能力采取“协议级声明禁用 + 最小状态桩”，避免客户端报错。

| Emby 服务域 | 建议策略 | 具体实现方式 |
| :--- | :--- | :--- |
| **HLS 转码**（`DynamicHlsService` / `HlsSegmentService`） | **能力声明禁用 + 兜底** | `PlaybackInfo` 声明 `SupportsTranscoding: false`、`SupportsDirectPlay: true`、`SupportsDirectStream: true`；`DELETE /Videos/ActiveEncodings` 返回 `204 No Content`（[Http Live Streaming](https://dev.emby.media/doc/restapi/Http-Live-Streaming.html)） |
| **Live TV / DLNA / Sync** | **最小状态桩** | `GET /LiveTv/Info` 返回 `IsEnabled: false`（[Live TV](https://dev.emby.media/doc/restapi/Live-TV.html)）；未启用能力不伪造业务数据 |
| **库刷新与计划任务**（`LibraryService` / `ScheduledTaskService`） | **桥接现有后台任务** | `POST /Library/Refresh` 触发 `rx-emby` 增量同步；`GET /ScheduledTasks` 映射展示同步与 TMDB 刮削任务状态 |

---

## 三、工程落地与数据表扩展规范

### 1. 代码模块规划（`src/web/emby_api/`）

- `auth.rs`：重构统一 `EmbyAuthContext` Extractor，新增 `/Users/Public`、`/System/Ping`、完整 `/System/Info`。
- `sessions.rs`：补齐 `/Sessions/Capabilities[/Full]`、`/Sessions/Logout`、`GET /Sessions`、`POST /Users/:uid/Items/:id/UserData`。
- `playback.rs`：补齐 `HEAD` 路由、`/Playback/BitrateTest`、`DELETE /Videos/ActiveEncodings` 兜底。
- `subtitles.rs`（新增）：实现 `SubtitleService` 外挂/内封字幕流输出。
- `metadata_lists.rs`（新增）：实现 `Genres`、`Persons`、`Studios`、`Artists`、`Years`、`Tags`、`OfficialRatings`、`Items/Counts`、`Items/:id/Similar`。
- `playlists.rs`（新增）：实现 `PlaylistService` 与 `CollectionService`。
- `display_prefs.rs`（新增）：实现 `DisplayPreferencesService`。
- `ws.rs`（新增）：实现 `/embywebsocket` 与 `/socket` WebSocket 连接管理与消息分发。

### 2. 数据库迁移扩展（`migrations/006_emby_compat.sql`）

建议在现有 PostgreSQL 16 结构基础上新增 4 张轻量表：
- `auth_tokens`：持久化多设备 `AccessToken`、`DeviceId`、`Client`、`LastActivityDate` 及吊销状态。
- `user_display_preferences`：`(user_id, client, display_prefs_id)` 唯一键存储客户端视图配置 JSONB。
- `playlists` & `playlist_items`：用户播放列表及有序条目表。
- `collections` & `collection_items`：自定义合集（BoxSet）及关联条目表。

### 3. 兼容性验证矩阵

1. **自动化契约测试**：基于 `axum` `oneshot` 扩展现有测试（参考 `src/web/smart_view_tests.rs`），验证认证头解析、HTTP 状态码、JSON `PascalCase` 字段及 Ticks（`1s = 10_000_000 ticks`）格式。
2. **未匹配路由观测**：持续监控 `src/web/mod.rs` 中的 `log_unmatched_404` 日志，使用 `VidHub`、`Infuse`、Emby 官方 Web / iOS / Android / TV 客户端实测，按真实 `404` 优先级收敛。
