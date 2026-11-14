<template>
  <section class="space-y-6">
    <div class="card p-6 rounded-2xl flex flex-wrap gap-4 items-center justify-between">
      <div>
        <h2 class="text-base font-bold text-white flex items-center gap-2">
          <Sparkles class="w-4 h-4 text-emerald-400" aria-hidden="true" />
          <span>智能视图管理</span>
        </h2>
        <p class="text-xs text-slate-400 mt-1">
          按地区、平台、流派和评分组合筛选规则，在 VidHub / Infuse 客户端首页生成专属分类栏目。
        </p>
      </div>
      <div class="flex gap-2.5">
        <button
          type="button"
          @click="handleSeedPresets"
          class="cursor-pointer bg-indigo-600/25 hover:bg-indigo-600/40 border border-indigo-500/40 text-indigo-200 px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
        >
          <Sparkles class="w-3.5 h-3.5 text-indigo-300" aria-hidden="true" />
          <span>一键生成 5 个预设视图</span>
        </button>
        <button
          type="button"
          @click="openModal()"
          class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 text-white px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
        >
          <Plus class="w-3.5 h-3.5" aria-hidden="true" />
          <span>新建智能视图</span>
        </button>
      </div>
    </div>

    <div v-if="loading" class="text-center py-12 text-slate-400 text-xs">正在加载智能视图…</div>
    <div
      v-else-if="libraries.length === 0"
      class="card p-12 rounded-2xl text-center space-y-3"
    >
      <div class="w-12 h-12 rounded-2xl bg-indigo-500/15 border border-indigo-500/30 text-indigo-300 flex items-center justify-center mx-auto">
        <Sparkles class="w-6 h-6" :stroke-width="1.75" aria-hidden="true" />
      </div>
      <div class="text-sm font-medium text-slate-200">尚未配置智能视图</div>
      <div class="text-xs text-slate-400">
        点击右上角「一键生成 5 个预设视图」，即可快速创建高分佳作、流媒体剧场、华语与日韩精选栏目
      </div>
    </div>
    <div v-else class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-5">
      <div
        v-for="v in libraries"
        :key="v.id"
        class="card card-interactive p-5 rounded-2xl flex flex-col justify-between space-y-4"
      >
        <div class="space-y-3">
          <div class="flex items-start justify-between gap-2">
            <div class="flex items-center gap-2.5 min-w-0">
              <span class="w-9 h-9 rounded-xl bg-slate-900 border border-slate-700/80 text-emerald-400 flex items-center justify-center shrink-0">
                <Film v-if="v.icon_type === 'movies'" class="w-4 h-4" :stroke-width="1.85" aria-hidden="true" />
                <Tv v-else class="w-4 h-4" :stroke-width="1.85" aria-hidden="true" />
              </span>
              <div class="min-w-0">
                <div class="font-bold text-sm text-white truncate">{{ v.name }}</div>
                <div class="text-[11px] font-mono text-slate-500 tabular-nums">
                  {{ v.view_id }} · 排序 #{{ v.sort_order }}
                </div>
              </div>
            </div>
            <span
              class="px-2 py-0.5 text-[10px] font-medium rounded-md shrink-0"
              :class="
                v.is_enabled
                  ? 'bg-emerald-950 text-emerald-300 border border-emerald-500/30'
                  : 'bg-slate-800 text-slate-400'
              "
            >
              {{ v.is_enabled ? '首页已挂载' : '已停用' }}
            </span>
          </div>

          <div class="px-3 py-2 rounded-xl bg-slate-950/70 border border-slate-800/80 flex items-center justify-between text-xs tabular-nums">
            <span class="text-slate-400">匹配影视数量</span>
            <span class="font-mono font-bold text-emerald-400">{{ v.matched_count }} 部</span>
          </div>

          <div class="flex flex-wrap gap-1.5">
            <template v-if="ruleBadges(v.filter_rules).length">
              <span
                v-for="(badge, idx) in ruleBadges(v.filter_rules)"
                :key="idx"
                class="px-2 py-0.5 rounded-md bg-slate-900 border border-slate-700/80 text-[11px] text-slate-300"
              >
                {{ badge }}
              </span>
            </template>
            <span v-else class="text-xs text-slate-500">无附加过滤条件（匹配全库）</span>
          </div>
        </div>

        <div class="flex gap-2 pt-3 border-t border-slate-800/70">
          <button
            type="button"
            @click="openModal(v)"
            :aria-label="`编辑智能视图 ${v.name} 规则`"
            class="cursor-pointer flex-1 py-1.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-xs font-medium text-slate-200 transition-colors flex items-center justify-center gap-1.5"
          >
            <SlidersHorizontal class="w-3.5 h-3.5 text-slate-400" aria-hidden="true" />
            <span>编辑规则</span>
          </button>
          <button
            type="button"
            @click="handleDelete(v)"
            :aria-label="`删除智能视图 ${v.name}`"
            class="cursor-pointer px-3.5 py-1.5 rounded-xl bg-rose-950/60 hover:bg-rose-900/80 border border-rose-800/50 text-xs font-medium text-rose-300 transition-colors flex items-center gap-1.5"
          >
            <Trash2 class="w-3.5 h-3.5" aria-hidden="true" />
            <span>删除</span>
          </button>
        </div>
      </div>
    </div>

    <!-- 智能视图可视化规则构建弹窗 -->
    <div
      v-if="modal.visible"
      role="dialog"
      aria-modal="true"
      aria-labelledby="vlib-modal-title"
      class="fixed inset-0 bg-black/75 backdrop-blur-xs flex items-center justify-center z-50 p-4"
      @click.self="modal.visible = false"
    >
      <div class="card p-6 rounded-2xl max-w-2xl w-full space-y-4 max-h-[92vh] overflow-y-auto">
        <div class="flex items-center justify-between border-b border-slate-800 pb-3">
          <h3 id="vlib-modal-title" class="text-base font-bold text-white flex items-center gap-2">
            <Sparkles class="w-4 h-4 text-emerald-400" aria-hidden="true" />
            <span>{{ modal.id ? '编辑智能视图规则' : '创建智能视图' }}</span>
          </h3>
          <button
            type="button"
            aria-label="关闭智能视图弹窗"
            @click="modal.visible = false"
            class="cursor-pointer text-slate-400 hover:text-white p-1 rounded-lg hover:bg-slate-800 transition-colors"
          >
            <X class="w-4 h-4" aria-hidden="true" />
          </button>
        </div>

        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4 text-xs">
          <div class="sm:col-span-2">
            <label for="vlib-name" class="block text-slate-300 mb-1.5">视图展示名称（在客户端首页显示）</label>
            <input
              id="vlib-name"
              v-model="modal.name"
              type="text"
              placeholder="例如：TMDB 8.0+ 高分佳作"
              class="w-full bg-slate-950 border border-slate-700 focus:border-emerald-500 rounded-xl px-3.5 py-2 text-sm"
            />
          </div>

          <div>
            <label for="vlib-icon-type" class="block text-slate-300 mb-1.5">客户端展示样式</label>
            <select
              id="vlib-icon-type"
              v-model="modal.iconType"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-sm"
            >
              <option value="tvshows">剧集样式</option>
              <option value="movies">电影样式</option>
            </select>
          </div>

          <div>
            <label for="vlib-sort-order" class="block text-slate-300 mb-1.5">客户端首页排序权重（越小越靠前）</label>
            <input
              id="vlib-sort-order"
              v-model.number="modal.sortOrder"
              type="number"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-sm font-mono tabular-nums"
            />
          </div>

          <div>
            <label for="vlib-media-type" class="block text-slate-300 mb-1.5">包含媒体类型</label>
            <select
              id="vlib-media-type"
              v-model="modal.mediaType"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-sm"
            >
              <option value="">不限（电影 + 剧集）</option>
              <option value="tv">仅剧集</option>
              <option value="movie">仅电影</option>
            </select>
          </div>

          <div>
            <label for="vlib-categories" class="block text-slate-300 mb-1.5">限定基础分类（逗号分隔或留空）</label>
            <input
              id="vlib-categories"
              v-model="modal.categoriesCsv"
              type="text"
              placeholder="例如：电视剧,动漫"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-sm"
            />
          </div>

          <!-- 国家/地区快捷点选 + 输入 -->
          <div class="sm:col-span-2 space-y-1.5">
            <label for="vlib-countries" class="block text-slate-300">制片国家或地区（可点击标签快速选择）</label>
            <div class="flex flex-wrap gap-1.5 pb-1">
              <button
                v-for="c in presetCountries"
                :key="c.code"
                type="button"
                :aria-pressed="hasCsvItem('countriesCsv', c.code)"
                @click="toggleCsvItem('countriesCsv', c.code)"
                class="cursor-pointer px-2.5 py-1 rounded-lg text-[11px] border transition-colors"
                :class="
                  hasCsvItem('countriesCsv', c.code)
                    ? 'bg-emerald-600/30 border-emerald-500 text-emerald-200'
                    : 'bg-slate-900 border-slate-800 text-slate-400 hover:text-slate-200'
                "
              >
                {{ c.label }} ({{ c.code }})
              </button>
            </div>
            <input
              id="vlib-countries"
              v-model="modal.countriesCsv"
              type="text"
              placeholder="例如：CN,HK,TW"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-xs font-mono"
            />
          </div>

          <!-- 播出平台快捷点选 + 输入 -->
          <div class="sm:col-span-2 space-y-1.5">
            <label for="vlib-networks" class="block text-slate-300">出品或播出平台（可点击标签快速选择）</label>
            <div class="flex flex-wrap gap-1.5 pb-1">
              <button
                v-for="net in presetNetworks"
                :key="net"
                type="button"
                :aria-pressed="hasCsvItem('networksCsv', net)"
                @click="toggleCsvItem('networksCsv', net)"
                class="cursor-pointer px-2.5 py-1 rounded-lg text-[11px] border transition-colors"
                :class="
                  hasCsvItem('networksCsv', net)
                    ? 'bg-indigo-600/30 border-indigo-500 text-indigo-200'
                    : 'bg-slate-900 border-slate-800 text-slate-400 hover:text-slate-200'
                "
              >
                {{ net }}
              </button>
            </div>
            <input
              id="vlib-networks"
              v-model="modal.networksCsv"
              type="text"
              placeholder="例如：Netflix,HBO,Apple TV+"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-xs"
            />
          </div>

          <div>
            <label for="vlib-genres" class="block text-slate-300 mb-1.5">流派标签（逗号分隔）</label>
            <input
              id="vlib-genres"
              v-model="modal.genresCsv"
              type="text"
              placeholder="例如：科幻,悬疑,犯罪"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-xs"
            />
          </div>

          <div>
            <label for="vlib-min-rating" class="block text-slate-300 mb-1.5">最低 TMDB 评分（0~10）</label>
            <input
              id="vlib-min-rating"
              v-model="modal.minRating"
              type="number"
              step="0.1"
              placeholder="例如：8.0"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-xs font-mono tabular-nums"
            />
          </div>

          <div>
            <label for="vlib-year-min" class="block text-slate-300 mb-1.5">最早发行年份</label>
            <input
              id="vlib-year-min"
              v-model="modal.yearMin"
              type="number"
              placeholder="例如：2024"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-xs font-mono tabular-nums"
            />
          </div>

          <div>
            <label for="vlib-year-max" class="block text-slate-300 mb-1.5">最晚发行年份</label>
            <input
              id="vlib-year-max"
              v-model="modal.yearMax"
              type="number"
              placeholder="例如：2026"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-xs font-mono tabular-nums"
            />
          </div>

          <div class="sm:col-span-2">
            <label for="vlib-keyword" class="block text-slate-300 mb-1.5">片名或路径包含关键词（可选）</label>
            <input
              id="vlib-keyword"
              v-model="modal.keyword"
              type="text"
              placeholder="例如：115分享 或 杜比视界"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-xs"
            />
          </div>

          <div class="sm:col-span-2 flex items-center gap-2 pt-1">
            <input
              id="vlib-enabled-cb"
              v-model="modal.isEnabled"
              type="checkbox"
              class="rounded bg-slate-900 border-slate-700"
            />
            <label for="vlib-enabled-cb" class="text-xs text-slate-300 cursor-pointer">
              在客户端首页展示该智能视图
            </label>
          </div>
        </div>

        <div class="flex justify-end gap-2.5 pt-3 border-t border-slate-800">
          <button
            type="button"
            @click="modal.visible = false"
            class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-4 py-2 rounded-xl text-xs font-medium transition-colors"
          >
            取消
          </button>
          <button
            type="button"
            @click="submitModal"
            class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 px-5 py-2 rounded-xl text-xs font-semibold text-white transition-colors"
          >
            保存并生效
          </button>
        </div>
      </div>
    </div>
  </section>
</template>

<script setup>
import { ref, reactive, onMounted } from 'vue'
import { Sparkles, Plus, Film, Tv, SlidersHorizontal, Trash2, X } from 'lucide-vue-next'
import { adminApi } from '../api/admin'
import { useToast, useConfirm } from '../composables/useToast'

const toast = useToast()
const confirmModal = useConfirm()
const loading = ref(false)
const libraries = ref([])

const presetCountries = [
  { code: 'CN', label: '中国大陆' },
  { code: 'HK', label: '中国香港' },
  { code: 'TW', label: '中国台湾' },
  { code: 'KR', label: '韩国' },
  { code: 'JP', label: '日本' },
  { code: 'US', label: '美国' },
  { code: 'GB', label: '英国' }
]

const presetNetworks = [
  'Netflix',
  'HBO',
  'Apple TV+',
  'Disney+',
  'Amazon',
  'Tencent Video',
  'iQiyi',
  'Youku',
  'Bilibili'
]

const modal = reactive({
  visible: false,
  id: null,
  name: '',
  iconType: 'tvshows',
  sortOrder: 10,
  mediaType: '',
  categoriesCsv: '',
  countriesCsv: '',
  networksCsv: '',
  genresCsv: '',
  minRating: '',
  yearMin: '',
  yearMax: '',
  keyword: '',
  isEnabled: true
})

function splitCsv(val) {
  return (val || '')
    .split(/[,，]/)
    .map(s => s.trim())
    .filter(Boolean)
}

function hasCsvItem(field, item) {
  return splitCsv(modal[field]).includes(item)
}

function toggleCsvItem(field, item) {
  const list = splitCsv(modal[field])
  const idx = list.indexOf(item)
  if (idx >= 0) {
    list.splice(idx, 1)
  } else {
    list.push(item)
  }
  modal[field] = list.join(',')
}

function ruleBadges(r = {}) {
  const badges = []
  if (r.media_type) badges.push(`类型: ${r.media_type === 'movie' ? '电影' : '剧集'}`)
  if (r.categories?.length) badges.push(`分类: ${r.categories.join('/')}`)
  if (r.countries?.length) badges.push(`国家: ${r.countries.join(',')}`)
  if (r.networks?.length) badges.push(`平台: ${r.networks.join(',')}`)
  if (r.genres?.length) badges.push(`流派: ${r.genres.join(',')}`)
  if (r.min_rating != null) badges.push(`评分 ≥ ${r.min_rating}`)
  if (r.year_min != null || r.year_max != null) {
    badges.push(`年份: ${r.year_min || '*'} ~ ${r.year_max || '*'}`)
  }
  if (r.keyword) badges.push(`关键词: "${r.keyword}"`)
  return badges
}

async function loadLibraries() {
  loading.value = true
  try {
    const data = await adminApi.listVirtualLibraries()
    libraries.value = data.libraries || []
  } catch (err) {
    toast.error('加载虚拟媒体库失败：' + err.message)
  } finally {
    loading.value = false
  }
}

function openModal(item = null) {
  const r = item ? item.filter_rules || {} : {}
  modal.id = item ? item.id : null
  modal.name = item ? item.name : ''
  modal.iconType = item ? item.icon_type : 'tvshows'
  modal.sortOrder = item ? item.sort_order : 10
  modal.mediaType = r.media_type || ''
  modal.categoriesCsv = (r.categories || []).join(',')
  modal.countriesCsv = (r.countries || []).join(',')
  modal.networksCsv = (r.networks || []).join(',')
  modal.genresCsv = (r.genres || []).join(',')
  modal.minRating = r.min_rating != null ? String(r.min_rating) : ''
  modal.yearMin = r.year_min != null ? String(r.year_min) : ''
  modal.yearMax = r.year_max != null ? String(r.year_max) : ''
  modal.keyword = r.keyword || ''
  modal.isEnabled = item ? item.is_enabled : true
  modal.visible = true
}

async function submitModal() {
  if (!modal.name.trim()) {
    return toast.warn('请输入智能视图名称')
  }
  const ratingVal = parseFloat(modal.minRating)
  const yMinVal = parseInt(modal.yearMin, 10)
  const yMaxVal = parseInt(modal.yearMax, 10)

  try {
    const res = await adminApi.saveVirtualLibrary({
      id: modal.id,
      name: modal.name.trim(),
      icon_type: modal.iconType,
      sort_order: Number(modal.sortOrder) || 0,
      is_enabled: modal.isEnabled,
      filter_rules: {
        media_type: modal.mediaType || null,
        categories: splitCsv(modal.categoriesCsv),
        countries: splitCsv(modal.countriesCsv),
        networks: splitCsv(modal.networksCsv),
        genres: splitCsv(modal.genresCsv),
        min_rating: !isNaN(ratingVal) ? ratingVal : null,
        year_min: !isNaN(yMinVal) ? yMinVal : null,
        year_max: !isNaN(yMaxVal) ? yMaxVal : null,
        keyword: modal.keyword.trim() || null
      }
    })
    toast.success(res.message)
    modal.visible = false
    await loadLibraries()
  } catch (err) {
    toast.error('保存失败：' + err.message)
  }
}

async function handleSeedPresets() {
  try {
    const data = await adminApi.seedPresetVirtualLibraries()
    toast.success(data.message)
    await loadLibraries()
  } catch (err) {
    toast.error('生成预设失败：' + err.message)
  }
}

async function handleDelete(v) {
  const ok = await confirmModal({
    title: '删除智能视图',
    message: `确认删除智能视图「${v.name}」吗？该操作不会影响在库影视资源。`,
    confirmText: '确认删除',
    danger: true
  })
  if (!ok) return
  try {
    await adminApi.deleteVirtualLibrary(v.id)
    toast.info(`已删除智能视图「${v.name}」`)
    await loadLibraries()
  } catch (err) {
    toast.error('删除失败：' + err.message)
  }
}

onMounted(loadLibraries)
</script>
