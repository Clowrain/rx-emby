# TMDB 智能视图可扩展字段核验

核验日期：2026-11-14。

## 优先补存详情字段，再增加专题数据

优先补充评分人数、原始语言、时长、剧集状态、制作公司和电影合集。这些字段已经包含在现有电影／剧集详情响应中；新刮削无需增加独立请求，只需接收并保存字段。历史条目需要重新获取详情才能补齐。关键词和演职员适合第二阶段，地区播放平台及分级适合按需要启用。

下表中的 API 字段已核对 TMDB 官方详情响应及 [OpenAPI 定义](https://developer.themoviedb.org/openapi/tmdb-api.json)。用途、优先级和刷新周期是本项目建议。

## 12 类值得增加的筛选维度

| 维度 | 字段及适用范围 | 获取方式 | 可形成的视图与条件 |
|---|---|---|---|
| 原始语言 | 电影／剧集 `original_language`；另存 `spoken_languages` | 原详情响应 | 日语原作、粤语电影、非英语佳作。原作语言与本地文件音轨分开 |
| 评分可信度 | 电影／剧集 `vote_count`，结合已有 `vote_average` | 原详情响应 | 评分 ≥ 8 且投票 ≥ 1,000，减少少量投票造成的虚高评分 |
| 热度 | 电影／剧集 `popularity` | 原详情响应 | 在库热门、按制片地区筛选后再按热度排序；作为排序项比固定阈值更实用 |
| 时长 | 电影 `runtime`；剧集 `episode_run_time` 为数组，分集可有独立 `runtime` | 原详情／季详情 | 90 分钟内电影、短时单集；明确“任意时长命中”或汇总口径 |
| 剧集状态和近期播出 | 剧集 `status`、`in_production`、`last_air_date`、`last_episode_to_air`、`next_episode_to_air` | 原详情响应 | 已完结、仍在制作、近期有新集、未来 7 天有播出计划 |
| 制作公司 | 电影／剧集 `production_companies` 的 ID 和名称 | 原详情响应 | 按某制作公司筛选，例如吉卜力专题；与播出网络分开 |
| 系列电影 | 电影 `belongs_to_collection` 的 ID 和名称 | 原详情响应 | 同系列电影合集、未归入合集的作品 |
| 剧集规模与类型 | 剧集 `number_of_seasons`、`number_of_episodes`、`type`、`seasons` | 原详情响应 | 单季短剧、多季长剧、按节目类型筛选；集数规则需要明确特别篇口径 |
| 题材关键词 | 电影／剧集关键词的 ID 和名称 | `keywords` 附加接口 | 时间旅行、反乌托邦等细分题材，弥补流派粒度过粗 |
| 演员、导演与创作者 | 电影 `credits`；剧集 `aggregate_credits`；剧集创作者 `created_by` 已在详情 | 附加接口；创作者无须额外接口 | 演员作品集、导演专题、创作者作品集 |
| 地区播放平台 | 电影／剧集 `watch/providers`，按地区和订阅／租赁／购买等方式组织 | 附加接口 | 某地区可在指定平台订阅观看的在库作品 |
| 分级和发行阶段 | 电影 `release_dates`；剧集 `content_ratings` | 附加接口 | 指定地区分级、近 30 天数字发行电影、院线上映专题 |

详情字段来源：[电影详情](https://developer.themoviedb.org/reference/movie-details)、[剧集详情](https://developer.themoviedb.org/reference/tv-series-details)、[季详情](https://developer.themoviedb.org/reference/tv-season-details)。

附加字段来源：[电影关键词](https://developer.themoviedb.org/reference/movie-keywords)、[剧集关键词](https://developer.themoviedb.org/reference/tv-series-keywords)、[电影演职员](https://developer.themoviedb.org/reference/movie-credits)、[剧集汇总演职员](https://developer.themoviedb.org/reference/tv-series-aggregate-credits)、[电影播放平台](https://developer.themoviedb.org/reference/movie-watch-providers)、[剧集播放平台](https://developer.themoviedb.org/reference/tv-series-watch-providers)、[电影发行日期与分级](https://developer.themoviedb.org/reference/movie-release-dates)、[剧集分级](https://developer.themoviedb.org/reference/tv-series-content-ratings)。

## 保持几个关键概念独立

### 播出网络、制作公司和地区播放平台

`networks` 是剧集详情字段；电影按制作公司筛选使用 `production_companies`。某个国家当前可在哪个平台观看，应使用按地区组织的 `watch/providers`，并区分订阅、租赁和购买。平台数据使用时应按官方要求标注 JustWatch 来源。[官方说明](https://developer.themoviedb.org/reference/movie-watch-providers)

规则分别提供“播出网络”“制作公司”“可观看平台”，不共用一个“平台”字段。平台规则要求先选地区。

### 已完结、近期播出和本地收齐

`status = Ended` 表达剧集状态；近期播出根据分集播出日期计算；本地收齐应将本地唯一的“季号＋集号”集合与 TMDB 对应季的目标分集集合比较。不同画质版本不能重复计算。按已播出部分计算时排除未来分集，并明确是否包含第 0 季特别篇。[剧集详情](https://developer.themoviedb.org/reference/tv-series-details)、[季详情](https://developer.themoviedb.org/reference/tv-season-details)

推荐拆成“作品已完结”“已播出集已收齐”“全剧已收齐”三个条件。缺少预期分集数据时保留未知状态。

### 热度与趋势榜

`popularity` 和日／周趋势榜使用不同计算方式。用详情里的热度排序时命名为“在库热门”；若要“今日趋势／本周趋势”，另接趋势接口并与本地 TMDB ID 集合求交集。[官方指标解释](https://developer.themoviedb.org/docs/popularity-and-trending)

### 原作语言与文件音轨

TMDB 原作语言描述作品，本地文件的国语配音、粤语音轨、双语版本应由本地媒体信息提供。语言专题可以同时支持这两类条件，但应清楚命名。

## 接收和存储建议

1. **保存稳定标识。** 流派、播出网络、制作公司、人物、关键词、合集使用 TMDB ID；国家保存 ISO 国家代码，语言保存 ISO 语言代码。名称用于展示，不作为规则的唯一匹配依据。电影和剧集的作品主键使用 `media_type + tmdb_id`。
2. **保留字段来源和刷新时间。** 详情、演职员、关键词、地区播放平台分别记录获取时间，避免一项附加数据失败就覆盖其他已成功的数据。
3. **附加响应按需合并。** 官方支持 `append_to_response` 将同命名空间下的子请求合并到详情响应，最多 20 项。电影可先增加 `keywords,credits`；剧集优先 `keywords,aggregate_credits`。[官方用法](https://developer.themoviedb.org/docs/append-to-response)
4. **适配响应结构差异。** 电影关键词数组名是 `keywords`，剧集是 `results`；TV 汇总演职员覆盖各集，而普通 TV credits 侧重最新季。不能直接复用电影反序列化结构。[电影关键词](https://developer.themoviedb.org/reference/movie-keywords)、[剧集关键词](https://developer.themoviedb.org/reference/tv-series-keywords)、[TV 汇总演职员](https://developer.themoviedb.org/reference/tv-series-aggregate-credits)
5. **明确缺失值。** 空语言、未知时长、未知状态不应转换成正常的零值或“已完结”。分级必须与地区一起保存；电影发行日期同时保留发行类型，数字发行对应类型 4。[发行类型说明](https://developer.themoviedb.org/reference/movie-release-dates)

## 刷新建议

建议采用以下可配置的初始策略：

- 热度、近期播出剧集的状态与下一集日期：每天刷新一次。
- 正在使用的地区播放平台数据：每日到每周刷新，并显示数据更新时间。
- 评分与投票人数：新片／热播条目每天刷新，旧作每周刷新。
- 语言、公司、合集、关键词、演职员：首次获取后低频刷新；手动刮削时允许更新。
- 历史数据补齐按唯一作品去重、限速和分批执行；同作品多个资源版本共享元数据。

## 规则能力应随字段一起扩展

新增字段之外，建议增加“任意命中／全部命中／排除”、数值区间、相对日期（最近 30 天）、条件分组（AND／OR）和结果排序。典型组合：

- **高分日语动画电影：** 电影＋日语原作＋动画流派＋评分 ≥ 8＋投票 ≥ 500。
- **已完结短剧：** 剧集＋Ended＋总集数 ≤ 12；按评分倒序。
- **某导演的非合集作品：** 导演 ID 命中＋无合集；按发行日期倒序。
- **本月新上数字版：** 指定发行地区＋数字发行日期在最近 30 天内。

匹配数量、客户端内容和动态封面选图应消费同一份规则解释，避免新增字段只对其中一个入口生效。

## 本地代码与数据核验（2026-11-14）

核验对象为当前工作区（包含未提交改动）及本地 `rx-emby-pg` 容器中的 `xiaoya_media` 数据库。查询使用只读事务；统计反映本次本地快照。影视去重口径为 `(tmdb_id, media_type)`，没有 TMDB ID 时按本地条目 ID 区分。

### 元数据覆盖决定视图当前能命中多少内容

| 指标 | 数量 |
| --- | ---: |
| `media_items` 目录条目 | 70,935 |
| 去重影视 | 48,377 |
| `scrape_status IN (1,3)` 的目录条目 | 21 |
| 上述已刮削条目对应的去重影视 | 13 |
| 具有国家、流派、首播日期、正评分的条目 | 各 21 |
| 具有播出网络的条目 | 13 |
| `media_items.tags` 非空条目 | 0 |
| `episodes` 物理源记录 | 658,712 |
| 具有正时长的物理源记录 | 19 |
| 具有播出日期的物理源记录 | 31 |

现有刮削仅保存名称、原名、年份、首播日期、简介、评分、图片、国家、播出网络、流派和基础分类。`genres.id` 用于推导基础分类后未保存；网络仅保存名称。见 [刮削数据结构](../src/scraper/tmdb.rs#L5)、[电影与剧集响应](../src/scraper/tmdb.rs#L39)、[入库更新](../src/scraper/mod.rs#L221)。

### 已确认的匹配错误与字段语义问题

1. **国家代码与中文存储值不一致。** 刮削将 CN/JP/US 等转换为中文；界面和预设规则使用 ISO 代码，查询却采用数组精确相交。本地 `CN/HK/TW` 命中 0 条，中文“中国大陆/中国香港/中国台湾”命中 7 条，均为剧集；5 个现有视图中有 2 个使用国家代码。应统一存 ISO 代码、界面翻译显示，并兼容转换已有中文数据和历史规则。见 [转换函数](../src/scraper/tmdb.rs#L360)、[地区选项](../frontend/src/components/VirtualLibrariesTab.vue#L672)、[筛选条件](../src/web/emby_api.rs#L577)。
2. **平台名称不稳定导致漏匹配。** 界面提交 `Apple TV+`，本地实际存 `Apple TV`，分别命中 0 条和 1 条。建议保存 network ID 与显示名，用 ID 匹配，并为历史名称提供映射。见 [平台选项](../frontend/src/components/VirtualLibrariesTab.vue#L682)、[网络响应字段](../src/scraper/tmdb.rs#L34)。
3. **`seasons.episode_count` 当前代表刮削尝试状态，不是真实集数。** 写入值为 `next_attempt`，另有 0（待处理）和 -1（季不存在）状态。新增“短剧/少集数/全集”条件前，必须把刮削尝试状态与 TMDB 集数拆开。见 [季更新](../src/scraper/mod.rs#L428)。
4. **已完成主条目不会自动补抓新增字段。** `scrape_status` 为 1/3 时提前返回，电视剧仅进入季集补齐；本地兄弟目录复用也会复制已有元数据。新增字段需要元数据版本、可恢复的去重补抓流程和适合动态字段的刷新策略。见 [已完成条目处理](../src/scraper/mod.rs#L151)、[兄弟目录复用](../src/scraper/mod.rs#L165)。

### 不增加 TMDB 接口即可扩展的条件

- **首播日期区间、最近 N 天首播：** 使用 `premiered_at`，先规范空值和日期格式；剧集首播日期与最近一集播出日期应分开。
- **评分区间、未评分、元数据完整性：** 现有评分可做上下限；当前缺评分被写为 0，不能据此区分真实零分和未知。需要投票数后再提供“可信高分”预设。
- **流派全部满足、排除地区/平台/流派、条件分组：** 现有数据可支持，规则层需补充 `any/all/not` 和分组语义；保留历史规则的字段间 AND、列表内 ANY 行为。
- **简介关键词：** `overview` 已保存，现有关键词仅检查标题、原名、根路径；可增加明确字段选择。
- **最近入库、已收藏、未看、继续观看：** 属于本地时间与用户状态，需和 TMDB 元数据区分；用户相关视图按当前用户求值，后台人数/数量预览必须标明用户上下文。
- **4K、杜比视界、REMUX、音轨标签：** 来自本地路径/文件名解析，不来自 TMDB。当前存于 `episodes.version_tag`，不在空的 `media_items.tags` 中。去重后，具有 4K 标签的影视为 6,689 部、杜比视界 2,487 部、REMUX 3,788 部。筛选可使用源记录存在性查询；同一版本的多个要求应在同一源记录上共同满足，避免把不同版本的能力拼在一起。剧集“存在 4K 集”和“全季都有 4K”应是不同条件。见 [版本解析](../src/sync/parser.rs#L325)、[物理源入库](../src/sync/batch_db.rs#L279)。

### 推荐的实施顺序

1. 修复地区与网络身份匹配，同时处理前次审计中的视图权限、类型覆盖和统计/列表/封面一致性。
2. 第一批补存原始语言、投票数、稳定流派/网络 ID、电影时长、剧集状态与类型、制作公司、电影合集，以及真实季集数量；详情刮削、兄弟复用和历史数据补抓同时接通。
3. 扩展规则为明确的字段、操作符和值；统一生成内容查询、去重计数和封面候选的过滤条件。新字段缺失时按未知处理，不默认满足筛选；相对日期使用明确时区和滚动窗口。
4. 上线高分且足够投票、短片、已完结、迷你剧、原始语言、制作公司、合集、4K/杜比等有明确数据来源的预设。
5. 再按实际需求补充人物、主题关键词、分级、地区播放平台，配套缓存与刷新频率。

“已完结”由 TMDB 剧集状态判断；“本地全集齐备”需要按季与集编号对照应有且已播出的集集合，并排除多版本重复。仅有 `Ended`、一个总集数或现有 `episode_count` 都不足以确定本地齐全。
