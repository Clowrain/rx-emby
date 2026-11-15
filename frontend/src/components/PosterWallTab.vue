<script setup>
import { ref, reactive, computed, watch, onMounted } from 'vue'
import {
  LayoutGrid,
  Film,
  Tv,
  Sparkles,
  Globe,
  Mic,
  CheckCircle2,
  Clock,
  AlertTriangle,
  ShieldAlert,
  Trash2,
  RotateCcw,
  Search,
  X,
  ChevronLeft,
  ChevronRight,
  ArrowUpDown,
  ArrowUp,
  ArrowDown,
  Link2,
  Send,
  Eye,
  EyeOff,
  FolderGit2
} from 'lucide-vue-next'
import { adminApi } from '../api/admin'
import { useToast } from '../composables/useToast'
import PosterCard from './poster-wall/PosterCard.vue'
import BindTmdbModal from './poster-wall/BindTmdbModal.vue'

const props = defineProps({
  initialCategory: { type: String, default: '' },
  initialStatus: { type: String, default: '' }
})

const emit = defineEmits(['open-detail'])

const toast = useToast()
const pageSize = 30
const offset = ref(0)
const total = ref(0)
const loading = ref(false)
const scrapingId = ref(null)
const binding = ref(false)
const items = ref([])
const stagingFolders = ref([])

const categoryTabs = [
  { value: '', label: '全部媒体', icon: LayoutGrid },
  { value: '电影', label: '电影', icon: Film },
  { value: '电视剧', label: '电视剧', icon: Tv },
  { value: '动漫', label: '动漫', icon: Sparkles },
  { value: '纪录片', label: '纪录片', icon: Globe },
  { value: '综艺', label: '综艺', icon: Mic }
]

const stagingStateTabs = [
  { key: 'pending', label: '待刮削', icon: Clock, isIgnored: 'false', state: 'pending' },
  { key: 'conflict', label: '校验存疑', icon: ShieldAlert, isIgnored: 'false', state: 'conflict' },
  { key: 'failed', label: '未匹配', icon: AlertTriangle, isIgnored: 'false', state: 'failed' },
  { key: 'ignored', label: '已屏蔽', icon: Trash2, isIgnored: 'true', state: 'all' },
  { key: 'all', label: '全部目录', icon: FolderGit2, isIgnored: 'false', state: 'all' }
]

const sortOptions = [
  { value: 'updated_at', label: '更新时间' },
  { value: 'created_at', label: '入库时间' },
  { value: 'year', label: '发行年份' },
  { value: 'rating', label: 'TMDB 评分' },
  { value: 'title', label: '片名' }
]

const filters = reactive({
  viewMode: 'works', // 'works' | 'staging'
  isIgnored: 'false',
  stagingState: 'pending',
  category: props.initialCategory || '',
  keyword: '',
  sortBy: 'updated_at',
  sortOrder: 'desc'
})

const bindModal = reactive({
  visible: false,
  mode: 'work', // 'work' | 'staging'
  targetId: null,
  rootDir: '',
  tmdbId: '',
  mediaType: ''
})

const currentPage = computed(() => Math.floor(offset.value / pageSize) + 1)
const totalPages = computed(() => Math.max(1, Math.ceil(total.value / pageSize)))

function displayTitle(item) {
  return item.title || item.parsed_title || `#${item.id}`
}

function switchViewMode(mode) {
  filters.viewMode = mode
  resetAndLoad()
}

function selectCategory(val) {
  filters.category = val
  resetAndLoad()
}

function selectStagingTab(st) {
  filters.isIgnored = st.isIgnored
  filters.stagingState = st.state
  resetAndLoad()
}

function handleSortFieldChange(val) {
  filters.sortBy = val
  filters.sortOrder = val === 'title' ? 'asc' : 'desc'
  resetAndLoad()
}

function toggleSortOrder() {
  filters.sortOrder = filters.sortOrder === 'asc' ? 'desc' : 'asc'
  resetAndLoad()
}

async function loadData({ silent = false } = {}) {
  const hasExisting =
    filters.viewMode === 'works' ? items.value.length > 0 : stagingFolders.value.length > 0
  if (!silent && !hasExisting) {
    loading.value = true
  }
  try {
    if (filters.viewMode === 'works') {
      const params = {
        limit: pageSize,
        offset: offset.value,
        sort_by: filters.sortBy,
        sort_order: filters.sortOrder
      }
      if (filters.category !== '') params.category = filters.category
      if (filters.keyword.trim() !== '') params.keyword = filters.keyword.trim()

      const data = await adminApi.listMedia(params)
      items.value = data.items || []
      total.value = data.total ?? items.value.length
    } else {
      const params = {
        limit: pageSize,
        offset: offset.value,
        is_ignored: filters.isIgnored,
        scrape_state: filters.stagingState
      }
      if (filters.category !== '') params.category = filters.category
      if (filters.keyword.trim() !== '') params.keyword = filters.keyword.trim()

      const data = await adminApi.listStagingFolders(params)
      stagingFolders.value = data.items || []
      total.value = data.total ?? stagingFolders.value.length
    }
  } catch (err) {
    toast.error('加载列表失败：' + err.message)
  } finally {
    loading.value = false
  }
}

function resetAndLoad() {
  offset.value = 0
  loadData()
}

function prevPage() {
  if (offset.value >= pageSize) {
    offset.value -= pageSize
    loadData()
  }
}

function nextPage() {
  if (offset.value + pageSize < total.value) {
    offset.value += pageSize
    loadData()
  }
}

async function handleScrapeWork(item) {
  scrapingId.value = item.id
  try {
    await adminApi.scrapeOne(item.id)
    toast.success(`《${displayTitle(item)}》分集元数据刷新成功`)
    await loadData({ silent: true })
  } catch (err) {
    toast.error('刷新失败：' + err.message)
  } finally {
    scrapingId.value = null
  }
}

async function handleToggleIgnoreWork(item) {
  try {
    const res = await adminApi.toggleIgnore(item.id, true)
    toast.info(res.message)
    await loadData({ silent: true })
  } catch (err) {
    toast.error('操作失败：' + err.message)
  }
}

async function handleScrapeStagingFolder(folder) {
  scrapingId.value = folder.id
  try {
    const res = await adminApi.scrapeStagingFolder(folder.id)
    if (res.scrape_state === 'verified') {
      toast.success(res.message)
    } else {
      toast.warn(res.message)
    }
    await loadData({ silent: true })
  } catch (err) {
    toast.error('刮削失败：' + err.message)
  } finally {
    scrapingId.value = null
  }
}

async function handlePublishWithoutTmdb(folder) {
  scrapingId.value = folder.id
  try {
    const res = await adminApi.promoteStagingFolder(folder.id, {
      tmdb_id: null,
      media_type: folder.media_type
    })
    toast.success(res.message)
    await loadData({ silent: true })
  } catch (err) {
    toast.error('发布失败：' + err.message)
  } finally {
    scrapingId.value = null
  }
}

async function handleToggleIgnoreStaging(folder) {
  try {
    const res = await adminApi.toggleIgnoreStagingFolder(folder.id, !folder.is_ignored)
    toast.info(res.message)
    await loadData({ silent: true })
  } catch (err) {
    toast.error('操作失败：' + err.message)
  }
}

async function handleBatchScrapeCurrentPage() {
  const ids = stagingFolders.value.filter(f => !f.is_ignored).map(f => f.id)
  if (ids.length === 0) return toast.warn('当前页没有可刮削目录')
  try {
    const res = await adminApi.batchScrapeStagingFolders(ids)
    toast.success(res.message)
  } catch (err) {
    toast.error('批量入队失败：' + err.message)
  }
}

async function handleRetryFailed() {
  try {
    const data = await adminApi.retryFailedScrapes()
    toast.success(data.message)
    await loadData({ silent: true })
  } catch (err) {
    toast.error('重置失败：' + err.message)
  }
}

function openBindWorkModal(item) {
  bindModal.mode = 'work'
  bindModal.targetId = item.id
  bindModal.rootDir = `正式作品 #${item.id} · ${item.title}`
  bindModal.tmdbId = item.tmdb_id || ''
  bindModal.mediaType = item.media_type || ''
  bindModal.visible = true
}

function openBindStagingModal(folder) {
  bindModal.mode = 'staging'
  bindModal.targetId = folder.id
  bindModal.rootDir = folder.folder_path
  bindModal.tmdbId = folder.hint_tmdb_id || ''
  bindModal.mediaType = folder.media_type || ''
  bindModal.visible = true
}

async function submitBindTmdb() {
  const tmdbId = parseInt(bindModal.tmdbId, 10)
  if (!tmdbId) {
    return toast.warn('请输入有效的数字 TMDB ID')
  }
  binding.value = true
  try {
    const payload = {
      tmdb_id: tmdbId,
      media_type: bindModal.mediaType || null
    }
    const res =
      bindModal.mode === 'staging'
        ? await adminApi.promoteStagingFolder(bindModal.targetId, payload)
        : await adminApi.bindTmdb(bindModal.targetId, payload)
    toast.success(res.message)
    bindModal.visible = false
    await loadData({ silent: true })
  } catch (err) {
    toast.error('绑定失败：' + err.message)
  } finally {
    binding.value = false
  }
}

function applyInitialFilters(cat, st) {
  filters.category = cat || ''
  if (st === '0' || st === 'pending') {
    filters.viewMode = 'staging'
    filters.stagingState = 'pending'
    filters.isIgnored = 'false'
  } else if (st === '2' || st === 'failed') {
    filters.viewMode = 'staging'
    filters.stagingState = 'failed'
    filters.isIgnored = 'false'
  } else if (st === 'conflict') {
    filters.viewMode = 'staging'
    filters.stagingState = 'conflict'
    filters.isIgnored = 'false'
  } else {
    filters.viewMode = 'works'
  }
}

watch(
  () => [props.initialCategory, props.initialStatus],
  ([cat, st]) => {
    applyInitialFilters(cat, st)
    resetAndLoad()
  }
)

onMounted(() => {
  applyInitialFilters(props.initialCategory, props.initialStatus)
  loadData()
})
</script>

<template>
  <section class="space-y-5">
    <!-- 顶部主视图切换：正式作品库 vs 中间表工作台 -->
    <div class="flex flex-wrap items-center justify-between gap-3">
      <div class="inline-flex items-center gap-1.5 bg-slate-950/90 p-1.5 rounded-2xl border border-slate-800">
        <button
          type="button"
          @click="switchViewMode('works')"
          class="cursor-pointer px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-2"
          :class="
            filters.viewMode === 'works'
              ? 'bg-emerald-600 text-white shadow-sm'
              : 'text-slate-400 hover:text-slate-200 hover:bg-slate-900'
          "
        >
          <CheckCircle2 class="w-4 h-4" aria-hidden="true" />
          <span>正式作品库 (海报墙)</span>
        </button>
        <button
          type="button"
          @click="switchViewMode('staging')"
          class="cursor-pointer px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-2"
          :class="
            filters.viewMode === 'staging'
              ? 'bg-indigo-600 text-white shadow-sm'
              : 'text-slate-400 hover:text-slate-200 hover:bg-slate-900'
          "
        >
          <FolderGit2 class="w-4 h-4" aria-hidden="true" />
          <span>待处理中间表 (人工刮削 / 校验工作台)</span>
        </button>
      </div>
    </div>

    <!-- 过滤控制台 -->
    <div class="card p-5 rounded-2xl space-y-4">
      <div class="flex flex-wrap items-center justify-between gap-4">
        <div role="group" aria-label="媒体分类过滤" class="flex flex-wrap items-center gap-1.5 bg-slate-950/80 p-1.5 rounded-xl border border-slate-800/80">
          <button
            v-for="cat in categoryTabs"
            :key="cat.value"
            type="button"
            @click="selectCategory(cat.value)"
            :aria-pressed="filters.category === cat.value"
            class="cursor-pointer px-3 py-1.5 rounded-lg text-xs font-medium transition-colors flex items-center gap-1.5"
            :class="
              filters.category === cat.value
                ? 'bg-emerald-600 text-white shadow-sm'
                : 'text-slate-400 hover:text-slate-200 hover:bg-slate-900'
            "
          >
            <component :is="cat.icon" class="w-3.5 h-3.5 shrink-0" :stroke-width="1.85" aria-hidden="true" />
            <span>{{ cat.label }}</span>
          </button>
        </div>

        <div class="flex items-center gap-2.5 flex-1 sm:flex-initial justify-end">
          <div class="relative w-full sm:w-72">
            <label for="poster-search-input" class="visually-hidden">搜索片名、TMDB ID 或目录路径</label>
            <Search class="w-3.5 h-3.5 text-slate-500 absolute left-3 top-1/2 -translate-y-1/2 pointer-events-none" aria-hidden="true" />
            <input
              id="poster-search-input"
              v-model="filters.keyword"
              @keydown.enter="resetAndLoad"
              type="search"
              placeholder="搜索片名、TMDB ID 或目录路径…"
              class="w-full bg-slate-950/90 border border-slate-700/80 focus:border-emerald-500 rounded-xl pl-8 pr-8 py-2 text-xs text-slate-100 transition-colors"
            />
            <button
              v-if="filters.keyword"
              type="button"
              aria-label="清空搜索关键词"
              @click="filters.keyword = ''; resetAndLoad()"
              class="cursor-pointer absolute right-2.5 top-1/2 -translate-y-1/2 text-slate-500 hover:text-slate-300 p-0.5 rounded"
            >
              <X class="w-3.5 h-3.5" aria-hidden="true" />
            </button>
          </div>
          <button
            type="button"
            @click="resetAndLoad"
            class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 px-4 py-2 rounded-xl text-xs font-semibold text-white transition-colors shrink-0 flex items-center gap-1.5"
          >
            <Search class="w-3.5 h-3.5" aria-hidden="true" />
            <span>搜索</span>
          </button>
        </div>
      </div>

      <!-- 第二行：Staging 子状态筛选 / 排序 / 分页 -->
      <div class="flex flex-wrap items-center justify-between gap-3 pt-3 border-t border-slate-800/70">
        <div v-if="filters.viewMode === 'staging'" class="flex flex-wrap items-center gap-2">
          <button
            v-for="st in stagingStateTabs"
            :key="st.key"
            type="button"
            @click="selectStagingTab(st)"
            class="cursor-pointer px-3 py-1 rounded-lg text-xs font-medium border transition-colors flex items-center gap-1.5"
            :class="
              (st.isIgnored === 'true' && filters.isIgnored === 'true') ||
              (st.isIgnored === 'false' && filters.isIgnored === 'false' && filters.stagingState === st.state)
                ? 'bg-slate-800 border-indigo-500/50 text-indigo-300'
                : 'bg-slate-950/50 border-slate-800 text-slate-400 hover:text-slate-200'
            "
          >
            <component :is="st.icon" class="w-3.5 h-3.5 shrink-0" :stroke-width="1.85" aria-hidden="true" />
            <span>{{ st.label }}</span>
          </button>

          <button
            type="button"
            @click="handleBatchScrapeCurrentPage"
            class="cursor-pointer ml-1 px-3 py-1 rounded-lg text-xs font-medium bg-emerald-600/20 hover:bg-emerald-600/35 text-emerald-300 border border-emerald-500/30 transition-colors flex items-center gap-1.5"
          >
            <Sparkles class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
            <span>批量刮削当前页</span>
          </button>

          <button
            type="button"
            @click="handleRetryFailed"
            class="cursor-pointer px-3 py-1 rounded-lg text-xs font-medium bg-amber-500/15 hover:bg-amber-500/25 text-amber-300 border border-amber-500/30 transition-colors flex items-center gap-1.5"
          >
            <RotateCcw class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
            <span>重试全部失败/存疑项</span>
          </button>
        </div>
        <div v-else class="text-xs text-slate-400">
          仅展示已通过校验并入库的正式作品（点击卡片可查看关联目录并调整多版本权重）
        </div>

        <div class="flex flex-wrap items-center gap-3 tabular-nums">
          <div
            v-if="filters.viewMode === 'works'"
            class="flex items-center gap-1.5 bg-slate-950/70 border border-slate-800 rounded-lg px-2 py-1"
          >
            <ArrowUpDown class="w-3.5 h-3.5 text-slate-400 shrink-0" aria-hidden="true" />
            <label for="poster-sort-select" class="visually-hidden">海报排序方式</label>
            <select
              id="poster-sort-select"
              :value="filters.sortBy"
              @change="handleSortFieldChange($event.target.value)"
              class="bg-transparent text-xs text-slate-200 focus:outline-none cursor-pointer pr-1"
            >
              <option v-for="opt in sortOptions" :key="opt.value" :value="opt.value" class="bg-slate-900 text-slate-200">
                {{ opt.label }}
              </option>
            </select>
            <button
              type="button"
              @click="toggleSortOrder"
              class="cursor-pointer p-1 rounded hover:bg-slate-800 text-slate-300 hover:text-emerald-400 transition-colors flex items-center gap-0.5 text-[11px]"
            >
              <ArrowUp v-if="filters.sortOrder === 'asc'" class="w-3.5 h-3.5" aria-hidden="true" />
              <ArrowDown v-else class="w-3.5 h-3.5" aria-hidden="true" />
              <span>{{ filters.sortOrder === 'asc' ? '升序' : '降序' }}</span>
            </button>
          </div>

          <span class="text-xs text-slate-400">
            共 <strong class="text-white font-mono">{{ total.toLocaleString() }}</strong> 项 · 第
            <strong class="text-emerald-400 font-mono">{{ currentPage }}</strong> / {{ totalPages }} 页
          </span>
          <div class="flex gap-1.5">
            <button
              type="button"
              @click="prevPage"
              :disabled="offset === 0"
              class="cursor-pointer bg-slate-900 hover:bg-slate-800 border border-slate-700/80 disabled:opacity-35 disabled:cursor-not-allowed px-2.5 py-1 rounded-lg text-xs transition-colors flex items-center gap-1"
            >
              <ChevronLeft class="w-3.5 h-3.5" aria-hidden="true" />
              <span>上一页</span>
            </button>
            <button
              type="button"
              @click="nextPage"
              :disabled="offset + pageSize >= total"
              class="cursor-pointer bg-slate-900 hover:bg-slate-800 border border-slate-700/80 disabled:opacity-35 disabled:cursor-not-allowed px-2.5 py-1 rounded-lg text-xs transition-colors flex items-center gap-1"
            >
              <span>下一页</span>
              <ChevronRight class="w-3.5 h-3.5" aria-hidden="true" />
            </button>
          </div>
        </div>
      </div>
    </div>

    <!-- 内容区：正式作品库海报墙 -->
    <template v-if="filters.viewMode === 'works'">
      <div v-if="loading" class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 2xl:grid-cols-6 gap-4">
        <div
          v-for="n in 12"
          :key="n"
          class="card rounded-2xl overflow-hidden animate-pulse h-80 bg-slate-900/50"
        ></div>
      </div>
      <div v-else-if="items.length === 0" class="card rounded-2xl text-center py-16 space-y-2.5">
        <div class="w-12 h-12 rounded-2xl bg-slate-900 border border-slate-800 flex items-center justify-center mx-auto text-slate-400">
          <Film class="w-6 h-6" :stroke-width="1.75" aria-hidden="true" />
        </div>
        <div class="text-sm font-medium text-slate-200">未找到已入库的正式作品</div>
        <div class="text-xs text-slate-400">可切换到「待处理中间表」查看待刮削、校验存疑或未匹配的目录</div>
      </div>
      <div v-else class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 2xl:grid-cols-6 gap-4">
        <PosterCard
          v-for="item in items"
          :key="item.id"
          :item="item"
          :scraping="scrapingId === item.id"
          @open-detail="emit('open-detail', $event)"
          @open-bind="openBindWorkModal"
          @scrape="handleScrapeWork"
          @toggle-ignore="handleToggleIgnoreWork"
        />
      </div>
    </template>

    <!-- 内容区：中间表 (Staging) 目录列表 -->
    <template v-else>
      <div v-if="loading" class="space-y-3">
        <div v-for="n in 6" :key="n" class="card rounded-2xl h-24 animate-pulse bg-slate-900/50"></div>
      </div>
      <div v-else-if="stagingFolders.length === 0" class="card rounded-2xl text-center py-16 space-y-2.5">
        <div class="w-12 h-12 rounded-2xl bg-slate-900 border border-slate-800 flex items-center justify-center mx-auto text-slate-400">
          <FolderGit2 class="w-6 h-6" :stroke-width="1.75" aria-hidden="true" />
        </div>
        <div class="text-sm font-medium text-slate-200">当前筛选条件下无中间表目录</div>
      </div>
      <div v-else class="space-y-3">
        <div
          v-for="f in stagingFolders"
          :key="f.id"
          class="card rounded-2xl p-4 flex flex-col lg:flex-row lg:items-center justify-between gap-4"
        >
          <div class="space-y-1.5 min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-2">
              <span class="font-semibold text-sm text-white">{{ f.parsed_title }}</span>
              <span v-if="f.parsed_year" class="text-xs font-mono text-slate-400">({{ f.parsed_year }})</span>
              <span class="px-2 py-0.5 rounded-md text-[11px] bg-slate-800 text-slate-300 border border-slate-700/60">
                {{ f.category }} · {{ f.media_type === 'movie' ? '电影' : '剧集' }}
              </span>
              <span class="px-2 py-0.5 rounded-md text-[11px] font-mono bg-emerald-950/80 text-emerald-300 border border-emerald-500/30">
                {{ f.file_count }} 个视频
              </span>
              <span v-if="f.hint_tmdb_id" class="px-2 py-0.5 rounded-md text-[11px] font-mono bg-sky-950/80 text-sky-300 border border-sky-500/30">
                提示 TMDB #{{ f.hint_tmdb_id }}
              </span>
              <span
                class="px-2 py-0.5 rounded-md text-[11px] font-medium"
                :class="{
                  'bg-slate-800 text-slate-300': f.scrape_state === 'pending',
                  'bg-amber-950 text-amber-300 border border-amber-500/40': f.scrape_state === 'conflict',
                  'bg-rose-950 text-rose-300 border border-rose-500/40': f.scrape_state === 'failed',
                  'bg-emerald-950 text-emerald-300 border border-emerald-500/40': f.scrape_state === 'verified'
                }"
              >
                {{ f.scrape_state }}
              </span>
            </div>

            <div class="text-xs font-mono text-slate-400 break-all">{{ f.folder_path }}</div>

            <div
              v-if="f.conflict_reason"
              class="text-xs text-amber-300 bg-amber-950/50 border border-amber-500/30 rounded-xl px-3 py-1.5"
            >
              校验冲突原因：{{ f.conflict_reason }}
            </div>
          </div>

          <div class="flex flex-wrap items-center gap-2 shrink-0">
            <button
              type="button"
              @click="handleScrapeStagingFolder(f)"
              :disabled="scrapingId === f.id"
              class="cursor-pointer px-3 py-1.5 rounded-xl bg-emerald-600/20 hover:bg-emerald-600/35 border border-emerald-500/35 text-emerald-300 text-xs font-medium flex items-center gap-1.5 disabled:opacity-50"
            >
              <Sparkles class="w-3.5 h-3.5" :class="{ 'animate-spin': scrapingId === f.id }" aria-hidden="true" />
              <span>{{ scrapingId === f.id ? '刮削中…' : '立即刮削' }}</span>
            </button>

            <button
              type="button"
              @click="openBindStagingModal(f)"
              class="cursor-pointer px-3 py-1.5 rounded-xl bg-indigo-600/20 hover:bg-indigo-600/35 border border-indigo-500/35 text-indigo-300 text-xs font-medium flex items-center gap-1.5"
            >
              <Link2 class="w-3.5 h-3.5" aria-hidden="true" />
              <span>绑定 TMDB 发布</span>
            </button>

            <button
              type="button"
              @click="handlePublishWithoutTmdb(f)"
              :disabled="scrapingId === f.id"
              class="cursor-pointer px-3 py-1.5 rounded-xl bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 text-xs font-medium flex items-center gap-1.5 disabled:opacity-50"
            >
              <Send class="w-3.5 h-3.5" aria-hidden="true" />
              <span>无 TMDB 直接发布</span>
            </button>

            <button
              type="button"
              @click="handleToggleIgnoreStaging(f)"
              class="cursor-pointer px-3 py-1.5 rounded-xl text-xs font-medium flex items-center gap-1.5"
              :class="
                f.is_ignored
                  ? 'bg-sky-600/25 hover:bg-sky-600/40 text-sky-300 border border-sky-500/30'
                  : 'bg-rose-950/60 hover:bg-rose-900/80 text-rose-300 border border-rose-800/50'
              "
            >
              <Eye v-if="f.is_ignored" class="w-3.5 h-3.5" aria-hidden="true" />
              <EyeOff v-else class="w-3.5 h-3.5" aria-hidden="true" />
              <span>{{ f.is_ignored ? '恢复' : '屏蔽' }}</span>
            </button>
          </div>
        </div>
      </div>
    </template>

    <BindTmdbModal
      :modal="bindModal"
      :binding="binding"
      @close="bindModal.visible = false"
      @submit="submitBindTmdb"
    />
  </section>
</template>
