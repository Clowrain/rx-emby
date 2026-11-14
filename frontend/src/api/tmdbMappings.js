// TMDB 原始值与中文展示名称映射（展示用中文，存储用 TMDB 原文）

export const COUNTRY_OPTIONS = [
  { value: 'CN', label: '中国大陆 (CN)' },
  { value: 'HK', label: '中国香港 (HK)' },
  { value: 'TW', label: '中国台湾 (TW)' },
  { value: 'US', label: '美国 (US)' },
  { value: 'JP', label: '日本 (JP)' },
  { value: 'KR', label: '韩国 (KR)' },
  { value: 'GB', label: '英国 (GB)' },
  { value: 'FR', label: '法国 (FR)' },
  { value: 'DE', label: '德国 (DE)' },
  { value: 'CA', label: '加拿大 (CA)' },
  { value: 'AU', label: '澳大利亚 (AU)' },
  { value: 'IN', label: '印度 (IN)' },
  { value: 'TH', label: '泰国 (TH)' },
  { value: 'SG', label: '新加坡 (SG)' },
  { value: 'ES', label: '西班牙 (ES)' },
  { value: 'IT', label: '意大利 (IT)' },
  { value: 'RU', label: '俄罗斯 (RU)' },
  { value: 'SE', label: '瑞典 (SE)' },
  { value: 'DK', label: '丹麦 (DK)' }
]

export const LANGUAGE_OPTIONS = [
  { value: 'zh', label: '中文 (zh)' },
  { value: 'cn', label: '粤语 (cn)' },
  { value: 'en', label: '英语 (en)' },
  { value: 'ja', label: '日语 (ja)' },
  { value: 'ko', label: '韩语 (ko)' },
  { value: 'fr', label: '法语 (fr)' },
  { value: 'de', label: '德语 (de)' },
  { value: 'es', label: '西班牙语 (es)' },
  { value: 'it', label: '意大利语 (it)' },
  { value: 'ru', label: '俄语 (ru)' },
  { value: 'th', label: '泰语 (th)' },
  { value: 'hi', label: '印地语 (hi)' },
  { value: 'pt', label: '葡萄牙语 (pt)' },
  { value: 'ar', label: '阿拉伯语 (ar)' },
  { value: 'vi', label: '越南语 (vi)' },
  { value: 'id', label: '印尼语 (id)' }
]

export const CATEGORY_OPTIONS = [
  { value: '电影', label: '电影' },
  { value: '电视剧', label: '电视剧' },
  { value: '动漫', label: '动漫' },
  { value: '纪录片', label: '纪录片' },
  { value: '综艺', label: '综艺' }
]

export const GENRE_OPTIONS = [
  { value: '剧情', label: '剧情' },
  { value: '喜剧', label: '喜剧' },
  { value: '动作', label: '动作' },
  { value: '科幻', label: '科幻' },
  { value: '悬疑', label: '悬疑' },
  { value: '犯罪', label: '犯罪' },
  { value: '动画', label: '动画' },
  { value: '奇幻', label: '奇幻' },
  { value: '爱情', label: '爱情' },
  { value: '惊悚', label: '惊悚' },
  { value: '冒险', label: '冒险' },
  { value: '家庭', label: '家庭' },
  { value: '战争', label: '战争' },
  { value: '历史', label: '历史' },
  { value: '恐怖', label: '恐怖' },
  { value: '纪录', label: '纪录' },
  { value: '音乐', label: '音乐' },
  { value: '西部', label: '西部' },
  { value: '动作冒险', label: '动作冒险' },
  { value: 'Sci-Fi & Fantasy', label: '科幻奇幻 (Sci-Fi & Fantasy)' },
  { value: 'War & Politics', label: '战争政治 (War & Politics)' },
  { value: '真人秀', label: '真人秀' },
  { value: '脱口秀', label: '脱口秀' },
  { value: '儿童', label: '儿童' },
  { value: '新闻', label: '新闻' }
]

export const NETWORK_PRESET_OPTIONS = [
  { value: 'Netflix', label: 'Netflix' },
  { value: 'HBO', label: 'HBO' },
  { value: 'HBO Max', label: 'HBO Max' },
  { value: 'Apple TV+', label: 'Apple TV+' },
  { value: 'Disney+', label: 'Disney+' },
  { value: 'Prime Video', label: 'Prime Video' },
  { value: 'Hulu', label: 'Hulu' },
  { value: 'BBC', label: 'BBC' },
  { value: 'NHK', label: 'NHK' },
  { value: 'tvN', label: 'tvN' },
  { value: 'JTBC', label: 'JTBC' },
  { value: '腾讯视频', label: '腾讯视频' },
  { value: '爱奇艺', label: '爱奇艺' },
  { value: '优酷', label: '优酷' },
  { value: '芒果TV', label: '芒果TV' },
  { value: '哔哩哔哩', label: '哔哩哔哩' }
]

export const CERTIFICATION_OPTIONS = [
  { value: 'G', label: '全年龄 (G)' },
  { value: 'PG', label: '家长指引 (PG)' },
  { value: 'PG-13', label: '13岁以上 (PG-13)' },
  { value: 'R', label: '限制级 (R)' },
  { value: 'NC-17', label: '17岁以上 (NC-17)' },
  { value: 'TV-Y', label: '幼龄 (TV-Y)' },
  { value: 'TV-G', label: '剧集全年龄 (TV-G)' },
  { value: 'TV-PG', label: '剧集家长指引 (TV-PG)' },
  { value: 'TV-14', label: '14岁以上剧集 (TV-14)' },
  { value: 'TV-MA', label: '成人剧集 (TV-MA)' },
  { value: 'IIA', label: '港片 IIA 级' },
  { value: 'IIB', label: '港片 IIB 级' },
  { value: 'III', label: '港片 III 级' }
]

export const VERSION_TAG_OPTIONS = [
  { value: '4K', label: '4K' },
  { value: '1080p', label: '1080p' },
  { value: 'REMUX', label: 'REMUX' },
  { value: 'HDR', label: 'HDR' },
  { value: '杜比视界', label: '杜比视界' },
  { value: 'Atmos', label: '杜比全景声 (Atmos)' },
  { value: '国语', label: '国语' },
  { value: '粤语', label: '粤语' },
  { value: '中字', label: '中字' }
]

const buildLabelMap = (opts) => Object.fromEntries(opts.map(o => [o.value.toUpperCase(), o.label.replace(/\s*\([^)]*\)$/, '')]))

export const COUNTRY_NAME_MAP = {
  ...buildLabelMap(COUNTRY_OPTIONS),
  UK: '英国'
}

export const LANGUAGE_NAME_MAP = Object.fromEntries(
  LANGUAGE_OPTIONS.map(o => [o.value.toLowerCase(), o.label.replace(/\s*\([^)]*\)$/, '')])
)

export const TV_TYPE_NAME_MAP = {
  Miniseries: '迷你剧',
  Scripted: '剧情类',
  Reality: '真人秀',
  Documentary: '纪录片',
  'Talk Show': '访谈',
  Animation: '动画'
}

export const WATCH_TYPE_NAME_MAP = {
  flatrate: '订阅流媒体',
  rent: '租赁',
  buy: '购买'
}
