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
  Lock,
  Trash2,
  RotateCcw,
  Search,
  X,
  ChevronLeft,
  ChevronRight
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

const categoryTabs = [
  { value: '', label: '全部媒体', icon: LayoutGrid },
  { value: '电影', label: '电影', icon: Film },
  { value: '电视剧', label: '电视剧', icon: Tv },
  { value: '动漫', label: '动漫', icon: Sparkles },
  { value: '纪录片', label: '纪录片', icon: Globe },
  { value: '综艺', label: '综艺', icon: Mic }
]

const statusTabs = [
  { key: 'all', label: '正常在库', icon: LayoutGrid, isIgnored: 'false', status: '' },
  { key: 'scraped', label: '已刮削', icon: CheckCircle2, isIgnored: 'false', status: '1' },
  { key: 'pending', label: '待刮削', icon: Clock, isIgnored: 'false', status: '0' },
  { key: 'failed', label: '未匹配', icon: AlertTriangle, isIgnored: 'false', status: '2' },
  { key: 'locked', label: '已锁定', icon: Lock, isIgnored: 'false', status: '3' },
  { key: 'ignored', label: '已屏蔽', icon: Trash2, isIgnored: 'true', status: '' }
]

const filters = reactive({
  isIgnored: 'false',
  scrapeStatus: props.initialStatus || '',
  category: props.initialCategory || '',
  keyword: ''
})

const activeStatusKey = computed(() => {
  if (filters.isIgnored === 'true') return 'ignored'
  if (filters.scrapeStatus === '1') return 'scraped'
  if (filters.scrapeStatus === '0') return 'pending'
  if (filters.scrapeStatus === '2') return 'failed'
  if (filters.scrapeStatus === '3') return 'locked'
  return 'all'
})

const bindModal = reactive({
  visible: false,
  mediaId: null,
  rootDir: '',
  tmdbId: '',
  mediaType: ''
})

const currentPage = computed(() => Math.floor(offset.value / pageSize) + 1)
const totalPages = computed(() => Math.max(1, Math.ceil(total.value / pageSize)))

function displayTitle(item) {
  return item.title || item.root_dir.split('/').pop()
}

function selectCategory(val) {
  filters.category = val
  resetAndLoad()
}

function selectStatusTab(st) {
  filters.isIgnored = st.isIgnored
  filters.scrapeStatus = st.status
  resetAndLoad()
}

async function loadMedia() {
  loading.value = true
  try {
    const params = {
      limit: pageSize,
      offset: offset.value,
      is_ignored: filters.isIgnored
    }
    if (filters.scrapeStatus !== '') params.scrape_status = filters.scrapeStatus
    if (filters.category !== '') params.category = filters.category
    if (filters.keyword.trim() !== '') params.keyword = filters.keyword.trim()

    const data = await adminApi.listMedia(params)
    items.value = data.items || []
    total.value = data.total ?? items.value.length
  } catch (err) {
    toast.error('加载媒体列表失败：' + err.message)
  } finally {
    loading.value = false
  }
}

function resetAndLoad() {
  offset.value = 0
  loadMedia()
}

function prevPage() {
  if (offset.value >= pageSize) {
    offset.value -= pageSize
    loadMedia()
  }
}

function nextPage() {
  if (offset.value + pageSize < total.value) {
    offset.value += pageSize
    loadMedia()
  }
}

async function handleScrapeOne(item) {
  scrapingId.value = item.id
  try {
    await adminApi.scrapeOne(item.id)
    toast.success(`《${displayTitle(item)}》TMDB 刮削成功`)
    await loadMedia()
  } catch (err) {
    toast.error('刮削失败：' + err.message)
  } finally {
    scrapingId.value = null
  }
}

async function handleToggleIgnore(item) {
  const nextIgnore = !item.is_ignored
  try {
    const res = await adminApi.toggleIgnore(item.id, nextIgnore)
    toast.info(res.message)
    await loadMedia()
  } catch (err) {
    toast.error('操作失败：' + err.message)
  }
}

async function handleRetryFailed() {
  try {
    const data = await adminApi.retryFailedScrapes()
    toast.success(data.message)
    await loadMedia()
  } catch (err) {
    toast.error('重置失败：' + err.message)
  }
}

function openBindModal(item) {
  bindModal.mediaId = item.id
  bindModal.rootDir = item.root_dir
  bindModal.tmdbId = item.tmdb_id || ''
  bindModal.mediaType = ''
  bindModal.visible = true
}

async function submitBindTmdb() {
  const tmdbId = parseInt(bindModal.tmdbId, 10)
  if (!tmdbId) {
    return toast.warn('请输入有效的数字 TMDB ID')
  }
  binding.value = true
  try {
    const res = await adminApi.bindTmdb(bindModal.mediaId, {
      tmdb_id: tmdbId,
      media_type: bindModal.mediaType || null
    })
    toast.success(res.message)
    bindModal.visible = false
    await loadMedia()
  } catch (err) {
    toast.error('绑定失败：' + err.message)
  } finally {
    binding.value = false
  }
}

watch(
  () => [props.initialCategory, props.initialStatus],
  ([cat, st]) => {
    filters.category = cat || ''
    filters.scrapeStatus = st || ''
    filters.isIgnored = 'false'
    resetAndLoad()
  }
)

onMounted(loadMedia)
</script>

<template>
  <section class="space-y-5">
    <!-- 顶部双层分段式过滤控制台 -->
    <div class="card p-5 rounded-2xl space-y-4">
      <!-- 第一行：分类分段胶囊 + 搜索框 -->
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

      <!-- 第二行：刮削状态与回收站切换 + 总数与分页器 -->
      <div class="flex flex-wrap items-center justify-between gap-3 pt-3 border-t border-slate-800/70">
        <div role="group" aria-label="刮削状态过滤" class="flex flex-wrap items-center gap-2">
          <button
            v-for="st in statusTabs"
            :key="st.key"
            type="button"
            @click="selectStatusTab(st)"
            :aria-pressed="activeStatusKey === st.key"
            class="cursor-pointer px-3 py-1 rounded-lg text-xs font-medium border transition-colors flex items-center gap-1.5"
            :class="
              activeStatusKey === st.key
                ? 'bg-slate-800 border-emerald-500/50 text-emerald-300'
                : 'bg-slate-950/50 border-slate-800 text-slate-400 hover:text-slate-200'
            "
          >
            <component :is="st.icon" class="w-3.5 h-3.5 shrink-0" :stroke-width="1.85" aria-hidden="true" />
            <span>{{ st.label }}</span>
          </button>

          <button
            type="button"
            @click="handleRetryFailed"
            class="cursor-pointer ml-1 px-3 py-1 rounded-lg text-xs font-medium bg-amber-500/15 hover:bg-amber-500/25 text-amber-300 border border-amber-500/30 transition-colors flex items-center gap-1.5"
          >
            <RotateCcw class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
            <span>重试全部未匹配项</span>
          </button>
        </div>

        <div class="flex items-center gap-3 tabular-nums">
          <span class="text-xs text-slate-400">
            共 <strong class="text-white font-mono">{{ total.toLocaleString() }}</strong> 部影视 · 第
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

    <!-- 海报墙网格 -->
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
      <div class="text-sm font-medium text-slate-200">未找到匹配的影视条目</div>
      <div class="text-xs text-slate-400">可尝试切换全部分类或清空搜索关键词</div>
    </div>
    <div v-else class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 2xl:grid-cols-6 gap-4">
      <PosterCard
        v-for="item in items"
        :key="item.id"
        :item="item"
        :scraping="scrapingId === item.id"
        @open-detail="emit('open-detail', $event)"
        @open-bind="openBindModal"
        @scrape="handleScrapeOne"
        @toggle-ignore="handleToggleIgnore"
      />
    </div>

    <BindTmdbModal
      :modal="bindModal"
      :binding="binding"
      @close="bindModal.visible = false"
      @submit="submitBindTmdb"
    />
  </section>
</template>
