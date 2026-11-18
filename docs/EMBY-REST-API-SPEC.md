# Emby Server REST API 接口规范与待完成接口文档

> 本文档整理自 [Emby Developer 官方文档](https://dev.emby.media/doc/index.html) 与 [Emby Server REST API 参考手册](https://dev.emby.media/reference/RestAPI.html)，包含 Emby Dev 协议规范、待实现的 P0–P3 接口详细定义，以及全量 70 个官方 Service 清单。

---

## 一、Emby Dev 核心协议规范

- **基础路径**：`http[s]://hostname:port/emby/{apipath}`（[REST API Access](https://dev.emby.media/doc/restapi/index.html)）。
- **请求/响应格式**：支持 `application/json` 与 `application/xml`，通过 `Content-Type` 请求头控制（[REST API Access](https://dev.emby.media/doc/restapi/index.html)）。
- **认证规范**：
  - **用户认证**（[User Authentication](https://dev.emby.media/doc/restapi/User-Authentication.html)）：请求头携带 `Authorization=Emby UserId="...", Client="...", Device="...", DeviceId="...", Version="..."`；登录 `/Users/AuthenticateByName` 后获得 `AccessToken`，后续请求通过 `X-Emby-Token` 传递。
  - **API Key 认证**（[API Key Authentication](https://dev.emby.media/doc/restapi/API-Key-Authentication.html)）：通过请求头 `X-Emby-Token` 或查询参数 `api_key=...` 传递。
- **WebSocket 协议**：支持通过 WebSocket 发送 `{MessageType: "ReportPlaybackProgress", Data: {...}}` 等消息，建议每 10 秒及播放器状态变化时上报（[Playback Check-ins](https://dev.emby.media/doc/restapi/Playback-Check-ins.html)）。

---

## 二、待完成核心 REST API 接口详细定义（按优先级）

> 以下列出 `rx-emby` 尚未实现或需补齐的重点兼容接口，供开发与联调直接对照使用。

### P0：握手、认证与流媒体探测

#### 1. `GET / HEAD / POST /emby/System/Ping`
- **所属服务**：`SystemService`（`getSystemPing` / `headSystemPing` / `postSystemPing`）
- **鉴权**：无需鉴权
- **响应**：`200 OK`，`text/plain` 或 JSON 字符串 `"Emby Server"`

#### 2. `GET /emby/System/Info`（独立完整系统信息）
- **所属服务**：`SystemService`（[getSystemInfo](https://dev.emby.media/reference/RestAPI/SystemService/getSystemInfo.html)）
- **鉴权**：需要 `X-Emby-Token` / `api_key`
- **说明**：区别于 `/System/Info/Public`，返回完整 `SystemInfo` 对象（含 `Id`、`ServerName`、`Version`、`OperatingSystem`、`HasPendingRestart`、`SupportsLibraryMonitor`、`WebSocketPortNumber` 等）。

#### 3. `GET /emby/Users/Public`
- **所属服务**：`UserService`（`getUsersPublic`）
- **鉴权**：无需鉴权（[User Authentication](https://dev.emby.media/doc/restapi/User-Authentication.html)）
- **响应**：`200 OK`，返回 `UserDto[]`（含 `Id`、`Name`、`ServerId`、`HasPassword`、`HasConfiguredPassword`、`PrimaryImageTag`）。

#### 4. `POST /emby/Sessions/Capabilities` & `POST /emby/Sessions/Capabilities/Full`
- **所属服务**：[SessionsService](https://dev.emby.media/reference/RestAPI/SessionsService.html)（`postSessionsCapabilities` / `postSessionsCapabilitiesFull`）
- **鉴权**：需要用户 Token
- **请求体（JSON）**：
  - `Capabilities`：`PlayableMediaTypes`、`SupportedCommands`、`SupportsMediaControl`、`SupportsSync` 等查询/表单字段
  - `Capabilities/Full`：完整 `ClientCapabilities` JSON（含 `DeviceProfile`、`DirectPlayProfiles`、`TranscodingProfiles`、`SubtitleProfiles`）
- **响应**：`204 No Content`

#### 5. `POST /emby/Sessions/Logout`
- **所属服务**：[SessionsService](https://dev.emby.media/reference/RestAPI/SessionsService.html)（`postSessionsLogout`）
- **鉴权**：需要用户 Token
- **响应**：`204 No Content`，吊销当前会话 Token

#### 6. `GET /emby/Playback/BitrateTest`
- **所属服务**：[MediaInfoService](https://dev.emby.media/reference/RestAPI/MediaInfoService.html)（`getPlaybackBitratetest`）
- **查询参数**：`Size`（整数，请求测试的字节数，如 `1000000`）
- **响应**：`200 OK`，返回指定长度二进制占位数据（`application/octet-stream`）

#### 7. `HEAD` 探测路由补全
- `HEAD /emby/Videos/:item_id/stream`（`headVideosByIdStream`）
- `HEAD /emby/Videos/:item_id/:file_name`（`headVideosByIdByStreamfilename`）
- `HEAD /emby/Items/:item_id/Images/:image_type`（`headItemsByIdImagesByType`）
- `HEAD /emby/Items/:item_id/Images/:image_type/:image_index`（`headItemsByIdImagesByTypeByIndex`）

---

### P1：媒体库统计、分类索引、字幕与 UserData

#### 1. `GET /emby/Items/Counts`
- **所属服务**：`LibraryService`（`getItemsCounts`）
- **查询参数**：`UserId`（可选）、`IsFavorite`（可选）
- **响应**：`ItemCounts` JSON（`MovieCount`、`SeriesCount`、`EpisodeCount`、`GameCount`、`ArtistCount`、`ProgramCount`、`TrailerCount`、`SongCount`、`AlbumCount`、`MusicVideoCount`、`BoxSetCount`、`BookCount`、`ItemCount`）

#### 2. `GET /emby/Items/:item_id/Similar`
- **所属服务**：`LibraryService`（`getItemsByIdSimilar`）
- **查询参数**：`UserId`、`Limit`、`Fields`
- **响应**：`QueryResult<BaseItemDto>`（`Items`、`TotalRecordCount`）

#### 3. 分类与标签检索端点
- `GET /emby/Genres` / `GET /emby/Genres/:name`（`GenresService`）
- `GET /emby/Persons` / `GET /emby/Persons/:name`（`PersonsService`）
- `GET /emby/Studios` / `GET /emby/Studios/:name`（`StudiosService`）
- `GET /emby/Artists` / `GET /emby/Artists/AlbumArtists` / `GET /emby/Artists/:name`（`ArtistsService`）
- `GET /emby/Years`、`GET /emby/Tags`、`GET /emby/OfficialRatings`（`TagService`、`OfficialRatingService`）
- `GET /emby/Localization/ParentalRatings`（`LocalizationService`）

#### 4. `GET / HEAD /emby/Videos/:id/:media_source_id/Subtitles/:index/Stream.:format`
- **所属服务**：`SubtitleService`（`getVideosByIdByMediasourceidSubtitlesByIndexStreamByFormat` 等）
- **路径参数**：`id`、`media_source_id`、`index`（字幕流索引）、`format`（`srt`、`vtt`、`ass`、`ssa`）
- **响应**：`200 OK`，对应字幕文件内容及 `Content-Type`（如 `text/vtt`、`application/x-subrip`）

#### 5. 用户头像与进度状态维护
- `GET / HEAD /emby/Users/:id/Images/:image_type`（`ImageService`）
- `POST /emby/Users/:user_id/Items/:item_id/UserData`（`PlaystateService`：`postUsersByUseridItemsByItemidUserdata`）
- `POST /emby/Users/:user_id/Items/:item_id/HideFromResume`（`UserLibraryService`：`postUsersByUseridItemsByIdHidefromresume`）

---

### P2：WebSocket、播放列表、合集与视图偏好

#### 1. `GET /embywebsocket` & `GET /socket`（WebSocket 升级）
- **Query 参数**：`api_key`、`deviceId`
- **上行消息**：`{ "MessageType": "ReportPlaybackStart" | "ReportPlaybackProgress" | "ReportPlaybackStopped" | "KeepAlive", "Data": { ... } }`
- **下行推送**：`Sessions`、`LibraryChanged`、`UserDataChanged`

#### 2. `DisplayPreferencesService`
- `GET /emby/DisplayPreferences/:display_preferences_id`（Query：`UserId`、`Client`）
- `POST /emby/DisplayPreferences/:display_preferences_id`（Body：`DisplayPreferences` JSON，返回 `204 No Content`）

#### 3. `PlaylistService`
- `POST /emby/Playlists`（创建播放列表，Query：`Name`、`Ids`、`UserId`、`MediaType`）
- `GET /emby/Playlists/:id/Items`
- `POST /emby/Playlists/:id/Items`（添加条目）
- `DELETE /emby/Playlists/:id/Items`（移除条目）
- `POST /emby/Playlists/:id/Items/:item_id/Move/:new_index`

#### 4. `CollectionService`
- `POST /emby/Collections`（创建合集，Query：`Name`、`Ids`、`ParentId`、`IsLocked`）
- `POST /emby/Collections/:id/Items`
- `DELETE /emby/Collections/:id/Items`

#### 5. 推荐发现
- `GET /emby/Users/:user_id/Suggestions`（`SuggestionsService`）
- `GET /emby/Movies/Recommendations`（`MoviesService`）

---

### P3：运维任务桥接与降级兜底

- `POST /emby/Library/Refresh`（`LibraryService`：`postLibraryRefresh`）→ 触发 `rx-emby` 增量同步
- `GET /emby/ScheduledTasks` & `POST /emby/ScheduledTasks/Running/:id`（`ScheduledTaskService`）→ 映射同步与刮削任务
- `GET /emby/System/ActivityLog/Entries`（[getSystemActivitylogEntries](https://dev.emby.media/reference/RestAPI/ActivityLogService/getSystemActivitylogEntries.html)）→ 操作审计日志
- `DELETE /emby/Videos/ActiveEncodings`（`HlsSegmentService`）→ 返回 `204 No Content`（[Http Live Streaming](https://dev.emby.media/doc/restapi/Http-Live-Streaming.html)）
- `GET /emby/LiveTv/Info`（`LiveTvService`）→ 返回 `{ "IsEnabled": false }`（[Live TV](https://dev.emby.media/doc/restapi/Live-TV.html)）

---

## 三、Emby Server REST API 全量 70 个 Service 索引

> 来源：[Emby Server REST API Reference](https://dev.emby.media/reference/RestAPI.html)。完整 OpenAPI v2/v3 定义亦包含在 [Emby SDK](https://dev.emby.media/home/sdk/index.html) 中。

1. **用户与会话**：`UserService`、`SessionsService`、`ConnectService`、`DisplayPreferencesService`、`ContentService`
2. **媒体库与检索**：`UserViewsService`、`ItemsService`、`UserLibraryService`、`TvShowsService`、`MoviesService`、`TrailersService`、`ChannelService`、`SuggestionsService`、`ArtistsService`、`PersonsService`、`GenresService`、`MusicGenresService`、`GameGenresService`、`StudiosService`、`OfficialRatingService`、`TagService`、`InstantMixService`
3. **媒体流与播放**：`MediaInfoService`、`VideoService`、`AudioService`、`UniversalAudioService`、`DynamicHlsService`、`VideoHlsService`、`HlsSegmentService`、`BifService`、`PlaystateService`
4. **图片、字幕与元数据**：`ImageService`、`RemoteImageService`、`SubtitleService`、`ItemLookupService`、`ItemRefreshService`、`ItemUpdateService`
5. **库管理、合集与播放列表**：`LibraryService`、`LibraryStructureService`、`CollectionService`、`PlaylistService`、`VideosService`
6. **直播电视与同步**：`LiveTvService`、`LiveStreamService`、`SyncService`、`PartyService`
7. **系统、编码与运维**：`SystemService`、`ConfigurationService`、`ActivityLogService`、`ScheduledTaskService`、`BackupApi`、`EnvironmentService`、`EncodingInfoService`、`CodecParameterService`、`FfmpegOptionsService`、`SubtitleOptionsService`、`ToneMapOptionsService`、`OpenApiService`
8. **设备、DLNA、通知与插件**：`DeviceService`、`DlnaService`、`DlnaServerService`、`NotificationsService`、`UserNotificationsService`、`PluginService`、`PackageService`、`BrandingService`、`LocalizationService`、`FeatureService`、`GenericUIApiService`、`WebAppService`
