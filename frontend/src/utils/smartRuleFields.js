import {
  COUNTRY_OPTIONS,
  LANGUAGE_OPTIONS,
  CATEGORY_OPTIONS,
  GENRE_OPTIONS,
  NETWORK_PRESET_OPTIONS,
  CERTIFICATION_OPTIONS,
  VERSION_TAG_OPTIONS
} from '../api/tmdbMappings.js'

const choices = (pairs) => pairs.map(([value, label]) => ({ value, label }))

export const SMART_RULE_SECTIONS = [
  {
    title: '基本筛选',
    fields: [
      { key: 'media_type', label: '媒体类型', shortLabel: '媒体类型', options: choices([['movie', '仅电影'], ['tv', '仅剧集']]) },
      { key: 'categories', label: '基础分类', shortLabel: '分类', presetOptions: CATEGORY_OPTIONS },
      { key: 'countries', label: '国家或地区', shortLabel: '地区', presetOptions: COUNTRY_OPTIONS, allowCustom: true, customPlaceholder: '其他代码如 SE' },
      { key: 'genres', label: '流派', shortLabel: '流派', presetOptions: GENRE_OPTIONS, allowCustom: true, customPlaceholder: '其他流派名' },
      { key: 'genres_all', label: '流派全匹配', type: 'checkbox', checkboxLabel: '所选流派必须全部满足' },
      { key: 'min_rating', label: '最低评分', shortLabel: '最低评分', type: 'number', step: 0.1, min: 0, max: 10 },
      { key: 'max_rating', label: '最高评分', type: 'number', step: 0.1, min: 0, max: 10 },
      { key: 'year_min', label: '最早发行年份', shortLabel: '发行年份≥', type: 'number', min: 0 },
      { key: 'year_max', label: '最晚发行年份', type: 'number', min: 0 },
      { key: 'networks', label: '播出网络名称', shortLabel: '播出网络', presetOptions: NETWORK_PRESET_OPTIONS, allowCustom: true, customPlaceholder: '其他网络如 BBC' }
    ]
  },
  {
    title: '语言、口碑与时长',
    fields: [
      { key: 'languages', label: '原始语言', shortLabel: '原始语言', presetOptions: LANGUAGE_OPTIONS, allowCustom: true, customPlaceholder: 'ISO代码如 pl', help: '作品原作语言（按 TMDB original_language 匹配）；本地配音请使用「资源标签」。' },
      { key: 'spoken_languages', label: '对白语言', shortLabel: '对白语言', presetOptions: LANGUAGE_OPTIONS, allowCustom: true, customPlaceholder: 'ISO代码如 pl' },
      { key: 'min_votes', label: '最低投票人数', type: 'number', min: 0 },
      { key: 'runtime_min', label: '最短时长（分钟）', type: 'number', min: 0 },
      { key: 'runtime_max', label: '最长时长（分钟）', type: 'number', min: 0, help: '剧集按 TMDB 提供的单集常用时长筛选。' },
      { key: 'min_popularity', label: '最低热度', type: 'number', step: 0.1, min: 0 },
      { key: 'sort_by', label: '默认结果排序（降序）', options: choices([['PremiereDate', '发行年份'], ['CommunityRating', '评分'], ['Popularity', '热度'], ['DateCreated', '入库时间'], ['SortName', '片名']]) }
    ]
  },
  {
    title: '剧集状态、播出与本地收齐',
    fields: [
      { key: 'statuses', label: '剧集状态', singleList: true, options: choices([['Ended', '已完结'], ['Returning Series', '持续更新'], ['Canceled', '已取消'], ['In Production', '制作中'], ['Planned', '计划中']]) },
      { key: 'tv_types', label: '节目类型', singleList: true, options: choices([['Miniseries', '迷你剧'], ['Scripted', '剧情类'], ['Reality', '真人秀'], ['Documentary', '纪录片'], ['Talk Show', '访谈'], ['Animation', '动画']]) },
      { key: 'seasons_min', label: '最少季数', type: 'number', min: 0 },
      { key: 'seasons_max', label: '最多季数', type: 'number', min: 0 },
      { key: 'episodes_min', label: '最少总集数', type: 'number', min: 0 },
      { key: 'episodes_max', label: '最多总集数', type: 'number', min: 0, help: '按 TMDB 总集数筛选；如需限制本地已收齐，请使用「本地收齐状态」。' },
      { key: 'local_complete', label: '本地收齐状态', options: choices([['aired', '已播出集已收齐（不含特别篇）'], ['all', '全剧已完结且收齐']]) },
      { key: 'aired_within_days', label: '最近多少天有新集播出', type: 'number', min: 1, max: 36500 },
      { key: 'airing_within_days', label: '未来多少天有播出计划', type: 'number', min: 0, max: 365, help: '0 表示今日即将播出，7 表示未来 7 天。' }
    ]
  },
  {
    title: '制作公司、播出网络、关键词与演职员',
    fields: [
      { key: 'network_ids', label: '播出网络（已刮削）', source: 'networks', help: '从已刮削 TMDB 数据中勾选，支持搜索筛选。' },
      { key: 'company_ids', label: '制作公司', source: 'companies' },
      { key: 'collection_ids', label: '电影合集', source: 'collections' },
      { key: 'has_collection', label: '是否属于电影合集', boolean: true, options: choices([['true', '属于某个合集'], ['false', '未归入合集']]) },
      { key: 'keyword_ids', label: '题材关键词', source: 'keywords' },
      { key: 'director_ids', label: '导演 / 创作者', source: 'directors' },
      { key: 'person_ids', label: '演员 / 导演 / 创作者', source: 'cast' }
    ]
  },
  {
    title: '地区播放平台与分级发行（JustWatch / TMDB）',
    fields: [
      { key: 'watch_region', label: '播放平台地区', options: COUNTRY_OPTIONS },
      { key: 'watch_types', label: '观看方式', singleList: true, options: choices([['flatrate', '订阅流媒体'], ['rent', '租赁'], ['buy', '购买']]) },
      { key: 'watch_provider_ids', label: '可观看平台', source: 'watch_providers', help: '平台数据源自 JustWatch（TMDB Watch Providers）。' },
      { key: 'cert_region', label: '分级 / 发行地区', options: COUNTRY_OPTIONS },
      { key: 'certifications', label: '内容分级', shortLabel: '分级', presetOptions: CERTIFICATION_OPTIONS, allowCustom: true, customPlaceholder: '其他分级如 18' },
      { key: 'digital_within_days', label: '最近多少天数字发行（电影）', type: 'number', min: 1, max: 36500 }
    ]
  },
  {
    title: '日期、关键词与资源版本',
    fields: [
      { key: 'premiered_after', label: '首播日期从', type: 'date' },
      { key: 'premiered_before', label: '首播日期至', type: 'date' },
      { key: 'premiered_within_days', label: '最近多少天首播', type: 'number', min: 1, max: 36500 },
      { key: 'added_within_days', label: '最近多少天入库', type: 'number', min: 1, max: 36500 },
      { key: 'keyword', label: '片名或路径包含', shortLabel: '关键词', placeholder: '输入片名或路径关键词' },
      { key: 'search_overview', label: '检索剧情简介', type: 'checkbox', checkboxLabel: '关键词同时检索剧情简介' },
      { key: 'version_tags', label: '资源版本标签', shortLabel: '资源标签', presetOptions: VERSION_TAG_OPTIONS, allowCustom: true, customPlaceholder: '其他标签回车添加', help: '同一物理文件须同时包含所选标签；剧集存在符合条件的单集即可匹配。' },
      { key: 'exclude_countries', label: '排除国家或地区', shortLabel: '排除地区', presetOptions: COUNTRY_OPTIONS, allowCustom: true, customPlaceholder: '其他代码如 SE' },
      { key: 'exclude_genres', label: '排除流派', shortLabel: '排除流派', presetOptions: GENRE_OPTIONS, allowCustom: true, customPlaceholder: '其他流派名' },
      { key: 'exclude_networks', label: '排除播出网络', shortLabel: '排除网络', presetOptions: NETWORK_PRESET_OPTIONS, allowCustom: true, customPlaceholder: '其他网络名' }
    ]
  }
]

export const ALL_SMART_RULE_FIELDS = SMART_RULE_SECTIONS.flatMap(s => s.fields)
export const SMART_RULE_FIELD_MAP = new Map(ALL_SMART_RULE_FIELDS.map(f => [f.key, f]))

const QUICK_KEYS = ['media_type', 'countries', 'genres', 'min_rating', 'year_min', 'networks', 'keyword', 'version_tags']
export const QUICK_SMART_RULE_FIELDS = QUICK_KEYS.map(k => SMART_RULE_FIELD_MAP.get(k)).filter(Boolean)

export function hasSmartRuleValue(field, val) {
  if (field.type === 'checkbox') return val === true
  if (Array.isArray(val)) return val.length > 0
  return val !== null && val !== undefined && val !== ''
}
