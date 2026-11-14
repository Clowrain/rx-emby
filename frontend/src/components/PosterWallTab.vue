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
      <div
        v-for="item in items"
        :key="item.id"
        class="card card-interactive rounded-2xl overflow-hidden flex flex-col justify-between group"
      >
        <!-- 海报主体与标题信息区 (点击直接打开分集与源明细抽屉) -->
        <button
          type="button"
          @click="$emit('open-detail', item.id)"
          class="cursor-pointer text-left w-full block focus-visible:outline-offset-[-2px]"
        >
          <div class="aspect-[2/3] bg-slate-950 relative overflow-hidden flex items-center justify-center">
            <img
              v-if="item.poster_path"
              :src="`/emby/Items/media_${item.id}/Images/Primary`"
              :alt="`${displayTitle(item)} 海报`"
              width="300"
              height="450"
              loading="lazy"
              class="w-full h-full object-cover group-hover:scale-105 transition-transform duration-500"
            />
            <div v-else class="px-4 text-center space-y-2">
              <Film class="w-8 h-8 text-slate-600 mx-auto" :stroke-width="1.5" aria-hidden="true" />
              <div class="text-xs text-slate-400 line-clamp-2 break-all">{{ displayTitle(item) }}</div>
            </div>

            <!-- 顶部状态与分类角标 (强制单行不折行，防碰撞间距) -->
            <div class="absolute inset-x-2 top-2 flex items-center justify-between gap-1.5 pointer-events-none">
              <div class="flex items-center gap-1 min-w-0">
                <span
                  class="px-2 py-0.5 text-[10px] font-medium rounded-md backdrop-blur-md shadow whitespace-nowrap shrink-0"
                  :class="statusBadgeClass(item.scrape_status)"
                >
                  {{ statusBadgeText(item.scrape_status) }}
                </span>
                <span
                  v-if="item.sibling_count > 1"
                  class="px-1.5 py-0.5 text-[10px] font-medium font-mono rounded-md bg-purple-950/90 text-purple-200 border border-purple-500/40 backdrop-blur-md whitespace-nowrap tabular-nums shrink-0"
                >
                  ×{{ item.sibling_count }} 目录
                </span>
              </div>

              <span class="px-2 py-0.5 text-[10px] rounded-md bg-slate-950/85 text-slate-200 backdrop-blur-md border border-slate-700/60 whitespace-nowrap shrink-0">
                {{ item.category }}
              </span>
            </div>

            <!-- 悬浮快捷查看源明细遮罩提示 -->
            <div class="absolute inset-0 bg-slate-950/45 opacity-0 group-hover:opacity-100 transition-opacity duration-200 flex items-center justify-center pointer-events-none">
              <span class="px-3 py-1.5 rounded-full bg-slate-950/90 border border-emerald-500/40 text-[11px] font-medium text-emerald-300 shadow-lg flex items-center gap-1.5">
                <Layers class="w-3.5 h-3.5" aria-hidden="true" />
                <span>查看源明细</span>
              </span>
            </div>

            <!-- 底部渐变遮罩与评分/源数 -->
            <div class="absolute inset-x-0 bottom-0 bg-gradient-to-t from-slate-950 via-slate-950/75 to-transparent pt-8 pb-2 px-2.5 flex items-center justify-between tabular-nums">
              <span
                v-if="item.rating && item.rating > 0"
                class="text-[11px] font-bold text-amber-400 drop-shadow flex items-center gap-1 whitespace-nowrap"
              >
                <Star class="w-3 h-3 fill-amber-400 text-amber-400 shrink-0" aria-hidden="true" />
                <span>{{ item.rating.toFixed(1) }}</span>
              </span>
              <span v-else class="text-[10px] text-slate-400 whitespace-nowrap">
                {{ item.year || '年份未知' }}
              </span>
              <span class="px-1.5 py-0.5 text-[10px] font-mono rounded-md bg-emerald-950/90 text-emerald-300 border border-emerald-500/30 whitespace-nowrap">
                {{ item.episode_count }} 源
              </span>
            </div>
          </div>

          <!-- 标题与单行路径（悬停自动平滑滚动展开完整路径） -->
          <div class="px-3 pt-2.5 pb-2 space-y-1">
            <div class="flex items-baseline justify-between gap-1.5 min-w-0">
              <span
                class="font-semibold text-xs text-slate-100 truncate group-hover:text-emerald-400 transition-colors"
              >
                {{ displayTitle(item) }}
              </span>
              <span v-if="item.year" class="text-[11px] text-slate-500 font-mono tabular-nums shrink-0">
                {{ item.year }}
              </span>
            </div>
            <div class="single-line-reveal text-[11px] text-slate-500 font-mono">
              <span>{{ item.root_dir }}</span>
            </div>
          </div>
        </button>

        <!-- 卡片底部单行精简工具栏（彻底消除折行与拥挤） -->
        <div class="px-2.5 py-2 border-t border-slate-800/60 bg-slate-950/40 flex items-center justify-between gap-1.5">
          <!-- 左侧：TMDB ID 绑定状态胶囊 -->
          <button
            type="button"
            @click="openBindModal(item)"
            :aria-label="item.tmdb_id ? `修改 ${displayTitle(item)} 的 TMDB ID (当前 #${item.tmdb_id})` : `为 ${displayTitle(item)} 手动绑定 TMDB ID`"
            class="cursor-pointer min-w-0 flex-1 h-7 px-2 rounded-lg bg-slate-900/90 hover:bg-slate-800 border border-slate-800/90 hover:border-slate-700 text-[11px] text-slate-300 hover:text-white font-mono transition-colors flex items-center gap-1.5 tabular-nums whitespace-nowrap overflow-hidden"
          >
            <Link2 class="w-3 h-3 text-emerald-400 shrink-0" aria-hidden="true" />
            <span class="truncate">{{ item.tmdb_id ? `#${item.tmdb_id}` : '绑定 TMDB' }}</span>
          </button>

          <!-- 右侧：高频快捷动作按钮组 -->
          <div class="flex items-center gap-1 shrink-0">
            <button
              type="button"
              @click="handleScrapeOne(item)"
              :disabled="scrapingId === item.id"
              :aria-label="`重新刮削 ${displayTitle(item)}`"
              class="cursor-pointer h-7 px-2.5 rounded-lg bg-emerald-600/20 hover:bg-emerald-600/35 border border-emerald-500/30 text-emerald-300 text-[11px] font-medium transition-colors disabled:opacity-50 flex items-center gap-1 whitespace-nowrap shrink-0"
            >
              <Sparkles class="w-3 h-3 shrink-0" :class="{ 'animate-spin': scrapingId === item.id }" aria-hidden="true" />
              <span>{{ scrapingId === item.id ? '刮削中' : '刮削' }}</span>
            </button>

            <button
              type="button"
              @click="handleToggleIgnore(item)"
              :aria-label="`${item.is_ignored ? '恢复' : '屏蔽'} ${displayTitle(item)}`"
              class="cursor-pointer h-7 w-7 rounded-lg transition-colors flex items-center justify-center shrink-0"
              :class="
                item.is_ignored
                  ? 'bg-sky-600/25 hover:bg-sky-600/40 text-sky-300 border border-sky-500/30'
                  : 'bg-rose-950/60 hover:bg-rose-900/80 text-rose-300 border border-rose-800/50'
              "
            >
              <Eye v-if="item.is_ignored" class="w-3.5 h-3.5" aria-hidden="true" />
              <EyeOff v-else class="w-3.5 h-3.5" aria-hidden="true" />
            </button>
          </div>
        </div>
      </div>
    </div>

    <!-- 手动绑定 TMDB ID 弹窗 -->
    <div
      v-if="bindModal.visible"
      role="dialog"
      aria-modal="true"
      aria-labelledby="bind-modal-title"
      class="fixed inset-0 bg-black/75 backdrop-blur-xs flex items-center justify-center z-50 p-4"
      @click.self="bindModal.visible = false"
    >
      <div class="card p-6 rounded-2xl max-w-md w-full space-y-4">
        <div class="flex items-center justify-between">
          <h3 id="bind-modal-title" class="text-base font-bold text-white flex items-center gap-2">
            <Link2 class="w-4 h-4 text-emerald-400" aria-hidden="true" />
            <span>手动绑定 TMDB ID</span>
          </h3>
          <button
            type="button"
            aria-label="关闭绑定弹窗"
            @click="bindModal.visible = false"
            class="cursor-pointer text-slate-400 hover:text-white p-1 rounded-lg hover:bg-slate-800 transition-colors"
          >
            <X class="w-4 h-4" aria-hidden="true" />
          </button>
        </div>
        <div class="p-3 rounded-xl bg-slate-950/80 border border-slate-800 text-xs font-mono text-slate-400 break-all">
          {{ bindModal.rootDir }}
        </div>
        <div class="space-y-3">
          <div>
            <label for="bind-tmdb-id" class="block text-xs text-slate-300 mb-1.5">TMDB 数字编号</label>
            <input
              id="bind-tmdb-id"
              v-model="bindModal.tmdbId"
              type="text"
              inputmode="numeric"
              placeholder="例如：279593"
              class="w-full bg-slate-950 border border-slate-700 focus:border-emerald-500 rounded-xl px-3.5 py-2 text-sm font-mono tabular-nums"
            />
          </div>
          <div>
            <label for="bind-media-type" class="block text-xs text-slate-300 mb-1.5">指定媒体类型（可选）</label>
            <select
              id="bind-media-type"
              v-model="bindModal.mediaType"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-sm"
            >
              <option value="">自动识别（推荐）</option>
              <option value="movie">电影</option>
              <option value="tv">剧集</option>
            </select>
          </div>
        </div>
        <div class="flex justify-end gap-2.5 pt-2">
          <button
            type="button"
            @click="bindModal.visible = false"
            class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-4 py-2 rounded-xl text-xs font-medium transition-colors"
          >
            取消
          </button>
          <button
            type="button"
            @click="submitBindTmdb"
            :disabled="binding"
            class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 px-4 py-2 rounded-xl text-xs font-semibold text-white transition-colors"
          >
            {{ binding ? '正在拉取 TMDB…' : '确认绑定并锁定' }}
          </button>
        </div>
      </div>
    </div>
  </section>
</template>

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
  Star,
  Link2,
  EyeOff,
  Eye,
  Layers,
  ChevronLeft,
  ChevronRight
} from 'lucide-vue-next'
import { adminApi } from '../api/admin'
import { useToast } from '../composables/useToast'

const props = defineProps({
  initialCategory: { type: String, default: '' },
  initialStatus: { type: String, default: '' }
})

defineEmits(['open-detail'])

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

function statusBadgeText(status) {
  return { 0: '待刮削', 1: '已刮削', 2: '未匹配', 3: '已锁定' }[status] || '未知'
}

function statusBadgeClass(status) {
  return (
    {
      0: 'bg-slate-800/90 text-slate-300 border border-slate-600/40',
      1: 'bg-emerald-950/90 text-emerald-300 border border-emerald-500/40',
      2: 'bg-rose-950/90 text-rose-300 border border-rose-500/40',
      3: 'bg-amber-950/90 text-amber-300 border border-amber-500/40'
    }[status] || 'bg-slate-800 text-slate-300'
  )
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
