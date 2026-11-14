<template>
  <section class="space-y-6">
    <!-- 顶部 4 大核心指标 KPI 卡片 (语义化 button 支持键盘导航与聚焦) -->
    <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-5">
      <!-- KPI 1: 在库影视资产 -->
      <button
        type="button"
        @click="$emit('navigate', { tab: 'poster' })"
        class="card card-interactive p-5 rounded-2xl text-left cursor-pointer relative overflow-hidden group"
      >
        <div class="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-emerald-500/80 to-transparent"></div>
        <div class="flex items-center justify-between">
          <span class="text-xs font-medium text-slate-400 group-hover:text-slate-200 transition-colors">在库影视总数</span>
          <span class="w-8 h-8 rounded-xl bg-emerald-500/15 border border-emerald-500/25 text-emerald-400 flex items-center justify-center">
            <Clapperboard class="w-4 h-4" :stroke-width="1.85" aria-hidden="true" />
          </span>
        </div>
        <div class="mt-3 flex items-baseline gap-2 tabular-nums">
          <span class="text-3xl font-bold text-white tracking-tight">
            {{ formatNum(overview?.media?.total_media) }}
          </span>
          <span class="text-xs text-slate-400">部影视</span>
        </div>
        <div class="mt-3 pt-3 border-t border-slate-800/80 flex items-center justify-between text-xs tabular-nums">
          <span class="text-slate-400">关联视频源</span>
          <span class="font-mono text-emerald-400 font-medium flex items-center gap-1">
            <span>{{ formatNum(overview?.media?.total_episodes) }} 个文件</span>
            <ArrowUpRight class="w-3.5 h-3.5 opacity-60 group-hover:opacity-100 transition-opacity" aria-hidden="true" />
          </span>
        </div>
      </button>

      <!-- KPI 2: TMDB 刮削覆盖率 -->
      <button
        type="button"
        @click="$emit('navigate', { tab: 'poster', status: '1' })"
        class="card card-interactive p-5 rounded-2xl text-left cursor-pointer relative overflow-hidden group"
      >
        <div class="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-cyan-500/80 to-transparent"></div>
        <div class="flex items-center justify-between">
          <span class="text-xs font-medium text-slate-400 group-hover:text-slate-200 transition-colors">TMDB 元数据刮削率</span>
          <span class="w-8 h-8 rounded-xl bg-cyan-500/15 border border-cyan-500/25 text-cyan-400 flex items-center justify-center">
            <Crosshair class="w-4 h-4" :stroke-width="1.85" aria-hidden="true" />
          </span>
        </div>
        <div class="mt-3 flex items-baseline gap-2 tabular-nums">
          <span class="text-3xl font-bold text-white tracking-tight">{{ scrapeRate }}%</span>
          <span class="text-xs text-emerald-400">
            已完成 {{ formatNum(overview?.media?.scraped_count) }} 部
          </span>
        </div>
        <div class="mt-3">
          <div class="h-1.5 w-full bg-slate-800 rounded-full overflow-hidden">
            <div
              class="h-full bg-gradient-to-r from-emerald-500 to-cyan-400 rounded-full transition-all duration-500"
              :style="{ width: `${scrapeRate}%` }"
            ></div>
          </div>
          <div class="mt-2 flex items-center justify-between text-[11px] text-slate-400 tabular-nums">
            <span>待刮削: {{ formatNum(overview?.media?.pending_count) }}</span>
            <span class="text-rose-400">未匹配: {{ formatNum(overview?.media?.failed_count) }}</span>
          </div>
        </div>
      </button>

      <!-- KPI 3: 实时并发播放与虚拟库 -->
      <button
        type="button"
        @click="$emit('navigate', { tab: 'users' })"
        class="card card-interactive p-5 rounded-2xl text-left cursor-pointer relative overflow-hidden group"
      >
        <div class="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-indigo-500/80 to-transparent"></div>
        <div class="flex items-center justify-between">
          <span class="text-xs font-medium text-slate-400 group-hover:text-slate-200 transition-colors">当前播放与智能视图</span>
          <span class="w-8 h-8 rounded-xl bg-indigo-500/15 border border-indigo-500/25 text-indigo-400 flex items-center justify-center">
            <Activity class="w-4 h-4" :stroke-width="1.85" aria-hidden="true" />
          </span>
        </div>
        <div class="mt-3 flex items-baseline gap-2 tabular-nums">
          <span class="text-3xl font-bold text-white tracking-tight">
            {{ overview?.system?.active_sessions || 0 }}
          </span>
          <span class="text-xs text-slate-400">路活跃直链流</span>
        </div>
        <div class="mt-3 pt-3 border-t border-slate-800/80 flex items-center justify-between text-xs tabular-nums">
          <span class="text-slate-400">智能视图 / 授权账号</span>
          <span class="font-mono text-indigo-300">
            {{ overview?.system?.vlib_count || 0 }} 视图 · {{ overview?.system?.user_count || 1 }} 用户
          </span>
        </div>
      </button>

      <!-- KPI 4: 本地图片缓存水位 -->
      <button
        type="button"
        @click="$emit('navigate', { tab: 'sync' })"
        class="card card-interactive p-5 rounded-2xl text-left cursor-pointer relative overflow-hidden group"
      >
        <div class="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-amber-500/80 to-transparent"></div>
        <div class="flex items-center justify-between">
          <span class="text-xs font-medium text-slate-400 group-hover:text-slate-200 transition-colors">本地图片缓存占用</span>
          <span class="w-8 h-8 rounded-xl bg-amber-500/15 border border-amber-500/25 text-amber-400 flex items-center justify-center">
            <HardDrive class="w-4 h-4" :stroke-width="1.85" aria-hidden="true" />
          </span>
        </div>
        <div class="mt-3 flex items-baseline gap-2 tabular-nums">
          <span class="text-3xl font-bold text-white tracking-tight">
            {{ overview?.cache?.used_mb || 0 }}
          </span>
          <span class="text-xs text-slate-400">/ {{ overview?.cache?.max_mb || 20480 }} MB</span>
        </div>
        <div class="mt-3 pt-3 border-t border-slate-800/80 flex items-center justify-between text-xs tabular-nums">
          <span class="text-slate-400">已缓存海报与剧照</span>
          <span class="font-mono text-amber-300">
            {{ formatNum(overview?.cache?.file_count) }} 张图片
          </span>
        </div>
      </button>
    </div>

    <!-- 中部：五大媒体库分类容量分布 + 快捷运营中心 -->
    <div class="grid grid-cols-1 lg:grid-cols-3 gap-6">
      <!-- 五大分类容量占比 -->
      <div class="card p-6 rounded-2xl lg:col-span-2 space-y-4">
        <div class="flex items-center justify-between">
          <div>
            <h2 class="text-base font-bold text-white">媒体库内容分类分布</h2>
            <p class="text-xs text-slate-400 mt-0.5">点击任意分类可查看对应海报墙</p>
          </div>
          <span class="text-xs text-slate-400 font-mono px-2.5 py-1 rounded-lg bg-slate-900/90 border border-slate-800">
            同 TMDB 跨目录已自动去重
          </span>
        </div>

        <div v-if="!overview?.media?.categories?.length" class="py-10 text-center text-xs text-slate-500">
          暂无分类统计数据
        </div>
        <div v-else class="space-y-3 pt-1">
          <button
            v-for="cat in overview.media.categories"
            :key="cat.category"
            type="button"
            @click="$emit('navigate', { tab: 'poster', category: cat.category })"
            class="w-full text-left group cursor-pointer p-2.5 -mx-2.5 rounded-xl hover:bg-slate-900/60 transition-colors space-y-1.5 block"
          >
            <div class="flex items-center justify-between text-xs tabular-nums">
              <span class="font-medium text-slate-200 group-hover:text-emerald-400 transition-colors flex items-center gap-2">
                <component
                  :is="categoryIconComponent(cat.category)"
                  class="w-4 h-4 text-emerald-400/90 shrink-0"
                  :stroke-width="1.85"
                  aria-hidden="true"
                />
                <span>{{ cat.category }}</span>
              </span>
              <span class="font-mono text-slate-400">
                <strong class="text-white">{{ formatNum(cat.count) }}</strong> 部 ({{ categoryPercent(cat.count) }}%)
              </span>
            </div>
            <div class="h-2 w-full bg-slate-900 rounded-full overflow-hidden border border-slate-800/80">
              <div
                class="h-full bg-gradient-to-r from-emerald-500 to-cyan-500 rounded-full transition-all duration-500"
                :style="{ width: `${Math.max(categoryPercent(cat.count), 2)}%` }"
              ></div>
            </div>
          </button>
        </div>
      </div>

      <!-- 快捷操作与最近同步状态 -->
      <div class="card p-6 rounded-2xl flex flex-col justify-between space-y-5">
        <div class="space-y-3">
          <div class="flex items-center justify-between">
            <h2 class="text-base font-bold text-white">最近一次同步状态</h2>
            <History class="w-4 h-4 text-slate-400" aria-hidden="true" />
          </div>
          <div
            v-if="overview?.last_sync"
            class="bg-slate-950/70 border border-slate-800/90 rounded-xl p-4 space-y-2.5 text-xs tabular-nums"
          >
            <div class="flex items-center justify-between">
              <span class="text-slate-400 font-mono">批次 #{{ overview.last_sync.id }}</span>
              <span
                class="px-2 py-0.5 rounded font-medium font-mono text-[11px]"
                :class="
                  overview.last_sync.status === 'success'
                    ? 'bg-emerald-950 text-emerald-300 border border-emerald-700/40'
                    : overview.last_sync.status === 'running'
                    ? 'bg-sky-950 text-sky-300 border border-sky-700/40'
                    : 'bg-rose-950 text-rose-300 border border-rose-700/40'
                "
              >
                {{ syncStatusText(overview.last_sync.status) }}
              </span>
            </div>
            <div class="text-slate-300 flex items-center gap-1.5">
              <Clock class="w-3.5 h-3.5 text-slate-500 shrink-0" aria-hidden="true" />
              <span>{{ new Date(overview.last_sync.started_at).toLocaleString() }}</span>
            </div>
            <div class="flex items-center justify-between text-slate-400 pt-2 border-t border-slate-800/80">
              <span>读取行数: {{ formatNum(overview.last_sync.total_lines) }}</span>
              <span>耗时: {{ (overview.last_sync.duration_ms || 0).toLocaleString() }} ms</span>
            </div>
            <div class="flex items-center gap-3 font-mono pt-0.5">
              <span class="text-emerald-400">新增 {{ overview.last_sync.added_episodes }} 个源</span>
              <span class="text-amber-400">移除 {{ overview.last_sync.deleted_episodes }} 个源</span>
              <span class="text-cyan-400">新增 {{ overview.last_sync.new_media_items }} 部影视</span>
            </div>
          </div>
          <div v-else class="text-xs text-slate-500 py-4">尚无同步记录</div>
        </div>

        <div class="space-y-2.5 pt-2">
          <div class="text-xs font-semibold text-slate-400">快捷操作</div>
          <div class="grid grid-cols-2 gap-2.5">
            <button
              type="button"
              @click="handleQuickSync"
              class="cursor-pointer p-2.5 rounded-xl bg-emerald-600/20 hover:bg-emerald-600/30 border border-emerald-500/30 text-emerald-300 text-xs font-medium transition-colors flex items-center gap-2 text-left"
            >
              <Zap class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
              <span>立即同步媒体库</span>
            </button>
            <button
              type="button"
              @click="$emit('navigate', { tab: 'poster', status: '2' })"
              class="cursor-pointer p-2.5 rounded-xl bg-rose-600/15 hover:bg-rose-600/25 border border-rose-500/30 text-rose-300 text-xs font-medium transition-colors flex items-center gap-2 text-left tabular-nums"
            >
              <SearchAlert class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
              <span class="truncate">处理未匹配项 ({{ overview?.media?.failed_count || 0 }})</span>
            </button>
            <button
              type="button"
              @click="$emit('navigate', { tab: 'vlib' })"
              class="cursor-pointer p-2.5 rounded-xl bg-indigo-600/15 hover:bg-indigo-600/25 border border-indigo-500/30 text-indigo-300 text-xs font-medium transition-colors flex items-center gap-2 text-left"
            >
              <Sparkles class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
              <span>管理智能视图</span>
            </button>
            <button
              type="button"
              @click="$emit('navigate', { tab: 'config' })"
              class="cursor-pointer p-2.5 rounded-xl bg-slate-800/90 hover:bg-slate-700 border border-slate-700 text-slate-200 text-xs font-medium transition-colors flex items-center gap-2 text-left"
            >
              <SlidersHorizontal class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
              <span>修改系统设置</span>
            </button>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>

<script setup>
import { computed } from 'vue'
import {
  Clapperboard,
  Crosshair,
  Activity,
  HardDrive,
  Film,
  Tv,
  Sparkles,
  Globe,
  Mic,
  Folder,
  ArrowUpRight,
  History,
  Clock,
  Zap,
  SearchAlert,
  SlidersHorizontal
} from 'lucide-vue-next'
import { adminApi } from '../api/admin'
import { useToast } from '../composables/useToast'

const props = defineProps({
  overview: {
    type: Object,
    default: null
  }
})

const emit = defineEmits(['navigate', 'refresh'])
const toast = useToast()

const scrapeRate = computed(() => {
  const total = props.overview?.media?.total_media || 0
  if (!total) return 0
  const scraped = props.overview?.media?.scraped_count || 0
  return Math.min(100, Math.round((scraped / total) * 100))
})

function formatNum(n) {
  return Number(n || 0).toLocaleString()
}

function syncStatusText(status) {
  return { success: '成功', running: '同步中', failed: '失败' }[status] || status
}

function categoryIconComponent(cat) {
  return (
    {
      电影: Film,
      电视剧: Tv,
      动漫: Sparkles,
      纪录片: Globe,
      综艺: Mic
    }[cat] || Folder
  )
}

function categoryPercent(count) {
  const total = props.overview?.media?.total_media || 0
  if (!total) return 0
  return Math.round((count / total) * 100)
}

async function handleQuickSync() {
  try {
    const res = await adminApi.triggerSync()
    toast.success(res.message)
    setTimeout(() => emit('refresh'), 1200)
  } catch (err) {
    toast.error('触发同步失败：' + err.message)
  }
}
</script>
