import { embyImageUrl } from '../api/admin.js'
import {
  COUNTRY_NAME_MAP,
  LANGUAGE_NAME_MAP,
  TV_TYPE_NAME_MAP,
  WATCH_TYPE_NAME_MAP
} from '../api/tmdbMappings.js'

export const collageOptions = [
  { key: 'collage_cards', label: '立体叠放卡片', desc: '模糊底图 + 微倾卡片' },
  { key: 'collage_diagonal', label: '斜切电影条带', desc: '4图13°斜切 + 暗角' },
  { key: 'collage_hero', label: '1主3辅非对称', desc: '左主剧照 + 右3缩略图' },
  { key: 'collage_blended', label: '羽化融合4连', desc: '4海报渐变无缝拼接' }
]

export const coverOptions = [
  { key: 'vlib_top_rated', label: '高分佳作' },
  { key: 'vlib_streaming', label: '流媒体剧场' },
  { key: 'vlib_chinese', label: '华语热播' },
  { key: 'vlib_kr_jp', label: '日韩精选' },
  { key: 'vlib_latest', label: '最新院线' },
  { key: 'vlib_default_movies', label: '电影视图' },
  { key: 'vlib_default_tv', label: '剧集视图' },
  { key: 'lib_movie', label: '影院胶片' },
  { key: 'lib_tv', label: '电视光影' },
  { key: 'lib_anime', label: '二次元星空' },
  { key: 'lib_doc', label: '自然山海' },
  { key: 'lib_variety', label: '舞台灯光' }
]

export function isDynamicCover(val) {
  return val === 'auto' || (typeof val === 'string' && val.startsWith('collage_'))
}

export function dynamicCoverLabel(val) {
  if (val === 'auto') return '动态首图'
  const found = collageOptions.find(o => o.key === val)
  return found ? found.label : '动态拼贴'
}

export function coverImageUrl(viewId, coverImage, fallbackKey, bust) {
  if (isDynamicCover(coverImage)) {
    return embyImageUrl(`/Items/${viewId}/Images/Primary?tag=${encodeURIComponent(coverImage)}&_t=${bust}`)
  }
  const key = coverImage || fallbackKey
  return `/covers/${key}.jpg?_t=${bust}`
}

export function formatCountries(list) {
  return list.map(code => COUNTRY_NAME_MAP[String(code).toUpperCase()] || code).join('、')
}

export function formatLanguages(list) {
  return list.map(code => LANGUAGE_NAME_MAP[String(code).toLowerCase()] || code).join('、')
}

export function resolveOptionNames(ruleOptions, sourceKey, ids) {
  const list = ruleOptions?.[sourceKey] || []
  const map = new Map(list.map(o => [o.id, o.name]))
  return ids.map(id => map.get(id) || `#${id}`).join('、')
}

const STATUS_LABELS = {
  Ended: '已完结',
  'Returning Series': '持续更新',
  Canceled: '已取消',
  'In Production': '制作中',
  Planned: '计划中'
}

const SORT_LABELS = {
  PremiereDate: '发行年份',
  CommunityRating: '评分',
  Popularity: '热度',
  DateCreated: '入库时间',
  SortName: '片名'
}

const OPTION_BADGE_FIELDS = [
  ['network_ids', 'networks', '播出网络'],
  ['company_ids', 'companies', '制作公司'],
  ['collection_ids', 'collections', '合集'],
  ['keyword_ids', 'keywords', '题材词'],
  ['director_ids', 'directors', '导演'],
  ['person_ids', 'cast', '演职员'],
  ['watch_provider_ids', 'watch_providers', '观看平台']
]

export function ruleBadges(r = {}, ruleOptions = {}) {
  const badges = []
  if (r.media_type) badges.push(`类型：${r.media_type === 'movie' ? '电影' : '剧集'}`)
  if (r.categories?.length) badges.push(`分类：${r.categories.join('、')}`)
  if (r.countries?.length) badges.push(`地区：${formatCountries(r.countries)}`)
  if (r.networks?.length) badges.push(`平台：${r.networks.join('、')}`)
  if (r.genres?.length) badges.push(`流派：${r.genres.join('、')}`)
  if (r.genres_all) badges.push('流派全匹配')
  if (r.min_rating != null) badges.push(`评分 ≥ ${r.min_rating}`)
  if (r.max_rating != null) badges.push(`评分 ≤ ${r.max_rating}`)
  if (r.year_min != null || r.year_max != null) {
    badges.push(`年份：${r.year_min || '*'} ~ ${r.year_max || '*'}`)
  }
  if (r.keyword) badges.push(`关键词："${r.keyword}"`)
  if (r.search_overview) badges.push('含剧情简介检索')
  if (r.languages?.length) badges.push(`语言：${formatLanguages(r.languages)}`)
  if (r.spoken_languages?.length) badges.push(`对白：${formatLanguages(r.spoken_languages)}`)
  if (r.min_votes != null) badges.push(`投票 ≥ ${r.min_votes}`)
  if (r.statuses?.length) {
    badges.push(`状态：${r.statuses.map(s => STATUS_LABELS[s] || s).join('、')}`)
  }
  if (r.tv_types?.length) {
    badges.push(`节目：${r.tv_types.map(t => TV_TYPE_NAME_MAP[t] || t).join('、')}`)
  }
  if (r.local_complete) badges.push(r.local_complete === 'all' ? '全剧已完结收齐' : '已播集已收齐')
  if (r.version_tags?.length) badges.push(`版本：${r.version_tags.join(' + ')}`)
  if (r.runtime_min != null) badges.push(`时长 ≥ ${r.runtime_min} 分钟`)
  if (r.runtime_max != null) badges.push(`时长 ≤ ${r.runtime_max} 分钟`)
  if (r.episodes_min != null) badges.push(`集数 ≥ ${r.episodes_min} 集`)
  if (r.episodes_max != null) badges.push(`集数 ≤ ${r.episodes_max} 集`)
  if (r.seasons_min != null) badges.push(`季数 ≥ ${r.seasons_min} 季`)
  if (r.seasons_max != null) badges.push(`季数 ≤ ${r.seasons_max} 季`)
  if (r.min_popularity != null) badges.push(`热度 ≥ ${r.min_popularity}`)
  if (r.premiered_within_days != null) badges.push(`近 ${r.premiered_within_days} 天首播`)
  if (r.added_within_days != null) badges.push(`近 ${r.added_within_days} 天入库`)
  if (r.aired_within_days != null) badges.push(`近 ${r.aired_within_days} 天有新集`)
  if (r.airing_within_days != null) {
    badges.push(r.airing_within_days === 0 ? '今日即将播出' : `未来 ${r.airing_within_days} 天播出`)
  }
  if (r.digital_within_days != null) badges.push(`近 ${r.digital_within_days} 天数字发行`)
  for (const [key, source, label] of OPTION_BADGE_FIELDS) {
    if (r[key]?.length) badges.push(`${label}：${resolveOptionNames(ruleOptions, source, r[key])}`)
  }
  if (r.watch_region) {
    badges.push(`平台地区：${COUNTRY_NAME_MAP[String(r.watch_region).toUpperCase()] || r.watch_region}`)
  }
  if (r.watch_types?.length) {
    badges.push(`观看方式：${r.watch_types.map(t => WATCH_TYPE_NAME_MAP[t] || t).join('、')}`)
  }
  if (r.cert_region || r.certifications?.length) {
    const regionName = r.cert_region
      ? (COUNTRY_NAME_MAP[String(r.cert_region).toUpperCase()] || r.cert_region)
      : ''
    badges.push(`分级：${[regionName, ...(r.certifications || [])].filter(Boolean).join(' ')}`)
  }
  if (r.exclude_countries?.length) badges.push(`排除地区：${formatCountries(r.exclude_countries)}`)
  if (r.exclude_genres?.length) badges.push(`排除流派：${r.exclude_genres.join('、')}`)
  if (r.exclude_networks?.length) badges.push(`排除网络：${r.exclude_networks.join('、')}`)
  if (r.has_collection != null) badges.push(r.has_collection ? '属于电影合集' : '未归入合集')
  if (r.premiered_after || r.premiered_before) {
    badges.push(`首播：${r.premiered_after || '*'} ~ ${r.premiered_before || '*'}`)
  }
  if (r.sort_by) badges.push(`排序：${SORT_LABELS[r.sort_by] || r.sort_by}`)
  if (r.any?.length) badges.push(`任选 ${r.any.length} 组条件`)
  return badges
}
