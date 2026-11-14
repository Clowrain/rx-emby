<template>
  <div class="min-h-dvh flex bg-[#060911] text-slate-100">
    <!-- 键盘无障碍跳转链接 -->
    <a href="#main-workspace" class="skip-link visually-hidden">跳转到主工作区</a>

    <!-- 左侧生产级导航边栏 -->
    <aside class="w-64 shrink-0 bg-[#080c16]/95 border-r border-slate-800/75 flex flex-col justify-between sticky top-0 h-dvh z-30">
      <div>
        <!-- 品牌 Logo 区 -->
        <div class="px-6 py-5 border-b border-slate-800/70 flex items-center gap-3.5">
          <div class="w-9 h-9 rounded-xl bg-gradient-to-br from-emerald-500 to-cyan-600 flex items-center justify-center shadow-lg shadow-emerald-950/80 ring-1 ring-white/15">
            <Play class="w-4 h-4 text-white fill-current ml-0.5" aria-hidden="true" />
          </div>
          <div>
            <div class="flex items-center gap-2">
              <span class="font-bold text-base tracking-tight text-white">RX-Emby</span>
              <span class="px-1.5 py-0.5 text-[10px] font-semibold font-mono rounded bg-emerald-500/15 text-emerald-400 border border-emerald-500/30">
                PRO
              </span>
            </div>
            <p class="text-[11px] text-slate-400 mt-0.5">零落盘直链流媒体服务</p>
          </div>
        </div>

        <!-- 分组菜单 -->
        <nav aria-label="主导航" class="p-3.5 space-y-5">
          <div v-for="group in navGroups" :key="group.title" class="space-y-1">
            <div class="px-3 text-[11px] font-semibold tracking-wide text-slate-500 mb-1.5">
              {{ group.title }}
            </div>
            <button
              v-for="item in group.items"
              :key="item.key"
              type="button"
              @click="switchTab(item.key)"
              :aria-current="activeTab === item.key ? 'page' : undefined"
              class="w-full flex items-center justify-between px-3 py-2.5 rounded-xl text-sm font-medium transition-colors duration-150 cursor-pointer"
              :class="
                activeTab === item.key
                  ? 'bg-gradient-to-r from-emerald-500/20 to-cyan-500/5 text-emerald-300 border border-emerald-500/35 shadow-sm'
                  : 'text-slate-400 hover:text-slate-100 hover:bg-slate-900/75 border border-transparent'
              "
            >
              <div class="flex items-center gap-3">
                <component
                  :is="item.icon"
                  class="w-4 h-4 shrink-0 transition-colors"
                  :class="activeTab === item.key ? 'text-emerald-400' : 'text-slate-400'"
                  :stroke-width="1.85"
                  aria-hidden="true"
                />
                <span>{{ item.label }}</span>
              </div>
              <span
                v-if="item.badge !== undefined && item.badge !== null"
                class="px-2 py-0.5 text-[10px] rounded-full font-mono tabular-nums"
                :class="
                  item.badgeActive
                    ? 'bg-emerald-500/20 text-emerald-300 border border-emerald-500/40'
                    : 'bg-slate-800/80 text-slate-400'
                "
              >
                {{ item.badge }}
              </span>
            </button>
          </div>
        </nav>
      </div>

      <!-- 底部引擎运行态卡片 -->
      <div class="p-4 border-t border-slate-800/70 space-y-2.5 bg-slate-950/60">
        <div class="flex items-center justify-between text-xs">
          <span class="flex items-center gap-2 text-slate-200 font-medium">
            <Radio class="w-3.5 h-3.5 text-emerald-400 animate-pulse" aria-hidden="true" />
            <span>302 直链转发引擎</span>
          </span>
          <span class="text-[10px] font-mono font-semibold px-1.5 py-0.5 rounded bg-emerald-500/15 text-emerald-400 border border-emerald-500/25">
            运行中
          </span>
        </div>
        <div class="text-[11px] text-slate-400 space-y-1 font-mono tabular-nums">
          <div class="truncate flex items-center gap-1.5" :title="overview?.system?.alist_base_url">
            <Server class="w-3 h-3 text-slate-500 shrink-0" aria-hidden="true" />
            <span class="truncate">{{ overview?.system?.alist_base_url || '正在连接网关…' }}</span>
          </div>
          <div class="flex items-center justify-between text-slate-500 pt-0.5">
            <span>TMDB 限流: {{ overview?.system?.tmdb_rate_limit || 8 }} req/s</span>
            <span class="text-slate-400">{{ overview?.system?.auto_background_scraper ? '自动扫描' : '按需刮削' }}</span>
          </div>
        </div>
      </div>
    </aside>

    <!-- 右侧主工作区 -->
    <div class="flex-1 flex flex-col min-w-0">
      <!-- 顶部状态与面包屑头栏 -->
      <header class="bg-[#080c16]/80 backdrop-blur-md border-b border-slate-800/80 px-8 py-4 flex flex-wrap items-center justify-between gap-4 sticky top-0 z-20">
        <div>
          <h1 id="main-workspace" tabindex="-1" class="text-lg font-bold text-white tracking-tight outline-none">
            {{ currentTabMeta.title }}
          </h1>
          <p class="text-xs text-slate-400 mt-0.5">{{ currentTabMeta.subtitle }}</p>
        </div>

        <div class="flex items-center gap-2.5">
          <button
            type="button"
            @click="switchTab('users')"
            class="cursor-pointer flex items-center gap-2 px-3 py-1.5 rounded-xl bg-slate-900/90 border border-slate-800 text-xs hover:border-emerald-500/40 transition-colors tabular-nums"
          >
            <Activity
              class="w-3.5 h-3.5"
              :class="(overview?.system?.active_sessions || 0) > 0 ? 'text-emerald-400 animate-pulse' : 'text-slate-500'"
              aria-hidden="true"
            />
            <span class="text-slate-400">并发播放流</span>
            <span class="font-semibold font-mono text-white">{{ overview?.system?.active_sessions || 0 }}</span>
          </button>

          <button
            type="button"
            @click="switchTab('sync')"
            class="cursor-pointer flex items-center gap-2 px-3 py-1.5 rounded-xl bg-slate-900/90 border border-slate-800 text-xs hover:border-cyan-500/40 transition-colors tabular-nums"
          >
            <HardDrive class="w-3.5 h-3.5 text-cyan-400" aria-hidden="true" />
            <span class="text-slate-400">图片缓存</span>
            <span class="font-mono text-emerald-400">
              {{ overview?.cache?.used_mb || 0 }} / {{ overview?.cache?.max_mb || 20480 }} MB
            </span>
          </button>

          <button
            type="button"
            @click="fetchOverview"
            class="cursor-pointer px-3 py-1.5 rounded-xl bg-slate-800/90 hover:bg-slate-700 border border-slate-700/70 text-xs font-medium text-slate-200 transition-colors flex items-center gap-1.5"
          >
            <RefreshCw class="w-3.5 h-3.5 text-slate-300" :class="{ 'animate-spin': refreshingOverview }" aria-hidden="true" />
            <span>刷新数据</span>
          </button>
        </div>
      </header>

      <!-- 主内容容器 -->
      <main class="flex-1 p-8 max-w-[1600px] w-full mx-auto">
        <OverviewDashboard
          v-if="activeTab === 'overview'"
          :overview="overview"
          @navigate="handleDashboardNavigate"
          @refresh="fetchOverview"
        />
        <PosterWallTab
          v-else-if="activeTab === 'poster'"
          :initial-category="posterInitialCategory"
          :initial-status="posterInitialStatus"
          @open-detail="drawerMediaId = $event"
        />
        <VirtualLibrariesTab v-else-if="activeTab === 'vlib'" />
        <UsersAndSessionsTab v-else-if="activeTab === 'users'" />
        <SyncAndCacheTab v-else-if="activeTab === 'sync'" />
        <HotConfigTab v-else-if="activeTab === 'config'" @saved="fetchOverview" />
      </main>
    </div>

    <!-- 右侧分集与多版本源抽屉 -->
    <MediaDetailDrawer :media-id="drawerMediaId" @close="drawerMediaId = null" />

    <!-- 全局生产级 Toast 通知 (无障碍 Polite Live Region) -->
    <div
      role="status"
      aria-live="polite"
      class="fixed top-5 right-5 z-50 space-y-2.5 max-w-sm w-full pointer-events-none"
    >
      <div
        v-for="t in toastState.items"
        :key="t.id"
        class="pointer-events-auto flex items-start gap-3 px-4 py-3 rounded-xl shadow-2xl border backdrop-blur-md text-sm transition-colors"
        :class="{
          'bg-emerald-950/95 border-emerald-500/40 text-emerald-100': t.type === 'success',
          'bg-rose-950/95 border-rose-500/40 text-rose-100': t.type === 'error',
          'bg-amber-950/95 border-amber-500/40 text-amber-100': t.type === 'warn',
          'bg-slate-900/95 border-slate-700 text-slate-100': t.type === 'info'
        }"
      >
        <CheckCircle2 v-if="t.type === 'success'" class="w-4 h-4 text-emerald-400 shrink-0 mt-0.5" aria-hidden="true" />
        <XCircle v-else-if="t.type === 'error'" class="w-4 h-4 text-rose-400 shrink-0 mt-0.5" aria-hidden="true" />
        <AlertTriangle v-else-if="t.type === 'warn'" class="w-4 h-4 text-amber-400 shrink-0 mt-0.5" aria-hidden="true" />
        <Info v-else class="w-4 h-4 text-sky-400 shrink-0 mt-0.5" aria-hidden="true" />
        <div class="flex-1 leading-snug break-all">{{ t.message }}</div>
      </div>
    </div>

    <!-- 全局生产级确认弹窗 -->
    <div
      v-if="confirmState.visible"
      role="dialog"
      aria-modal="true"
      aria-labelledby="confirm-dialog-title"
      class="fixed inset-0 bg-black/75 backdrop-blur-xs flex items-center justify-center z-50 p-4"
      @click.self="resolveConfirm(false)"
    >
      <div class="card p-6 rounded-2xl max-w-md w-full space-y-4">
        <div class="flex items-start gap-3">
          <div
            class="w-9 h-9 rounded-xl flex items-center justify-center shrink-0"
            :class="confirmState.danger ? 'bg-rose-500/15 text-rose-400 border border-rose-500/30' : 'bg-emerald-500/15 text-emerald-400 border border-emerald-500/30'"
          >
            <AlertTriangle v-if="confirmState.danger" class="w-5 h-5" aria-hidden="true" />
            <Info v-else class="w-5 h-5" aria-hidden="true" />
          </div>
          <div class="space-y-1">
            <h2 id="confirm-dialog-title" class="text-base font-bold text-white">{{ confirmState.title }}</h2>
            <p class="text-sm text-slate-300 leading-relaxed">{{ confirmState.message }}</p>
          </div>
        </div>
        <div class="flex justify-end gap-2.5 pt-2">
          <button
            type="button"
            @click="resolveConfirm(false)"
            class="cursor-pointer px-4 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 text-xs font-medium text-slate-200 transition-colors"
          >
            {{ confirmState.cancelText }}
          </button>
          <button
            type="button"
            @click="resolveConfirm(true)"
            class="cursor-pointer px-4 py-2 rounded-xl text-xs font-semibold text-white transition-colors"
            :class="
              confirmState.danger
                ? 'bg-rose-600 hover:bg-rose-500'
                : 'bg-emerald-600 hover:bg-emerald-500'
            "
          >
            {{ confirmState.confirmText }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import {
  Play,
  LayoutDashboard,
  Film,
  Sparkles,
  Users,
  RefreshCw,
  SlidersHorizontal,
  Radio,
  Server,
  Activity,
  HardDrive,
  CheckCircle2,
  XCircle,
  AlertTriangle,
  Info
} from 'lucide-vue-next'
import { adminApi } from './api/admin'
import { toastState, confirmState, resolveConfirm } from './composables/useToast'
import OverviewDashboard from './components/OverviewDashboard.vue'
import PosterWallTab from './components/PosterWallTab.vue'
import MediaDetailDrawer from './components/MediaDetailDrawer.vue'
import VirtualLibrariesTab from './components/VirtualLibrariesTab.vue'
import UsersAndSessionsTab from './components/UsersAndSessionsTab.vue'
import SyncAndCacheTab from './components/SyncAndCacheTab.vue'
import HotConfigTab from './components/HotConfigTab.vue'

const activeTab = ref('overview')
const drawerMediaId = ref(null)
const overview = ref(null)
const refreshingOverview = ref(false)
const posterInitialCategory = ref('')
const posterInitialStatus = ref('')

const tabMetas = {
  overview: {
    title: '系统运行总览',
    subtitle: '实时查看媒体库规模、TMDB 元数据刮削进度、当前播放会话与缓存占用'
  },
  poster: {
    title: '媒体库与海报墙',
    subtitle: '浏览海报墙、查看多版本片源、手动修正 TMDB 匹配或屏蔽指定条目'
  },
  vlib: {
    title: '智能视图',
    subtitle: '按评分、地区、平台或流派组合规则，在 VidHub / Infuse 客户端首页生成专属分类栏目'
  },
  users: {
    title: '用户权限与实时会话',
    subtitle: '管理家庭成员账号与分类访问权限，查看实时播放会话与已登录设备'
  },
  sync: {
    title: '媒体同步与图片缓存',
    subtitle: '管理 strm.txt 索引同步任务、查看同步记录并控制本地海报缓存空间'
  },
  config: {
    title: '系统设置',
    subtitle: '配置 AList 服务连接、TMDB 刮削代理、同步过滤规则与图片缓存上限，保存后立即生效'
  }
}

const currentTabMeta = computed(() => tabMetas[activeTab.value] || tabMetas.overview)

watch(
  currentTabMeta,
  meta => {
    document.title = `${meta.title} | RX-Emby 管理后台`
  },
  { immediate: true }
)

const navGroups = computed(() => [
  {
    title: '内容与视图运营',
    items: [
      { key: 'overview', label: '数据总览', icon: LayoutDashboard },
      {
        key: 'poster',
        label: '媒体库海报墙',
        icon: Film,
        badge: overview.value?.media?.total_media ?? null
      },
      {
        key: 'vlib',
        label: '智能视图',
        icon: Sparkles,
        badge: overview.value?.system?.vlib_count ?? null
      }
    ]
  },
  {
    title: '系统与会话管控',
    items: [
      {
        key: 'users',
        label: '用户与活跃会话',
        icon: Users,
        badge: overview.value?.system?.active_sessions || null,
        badgeActive: (overview.value?.system?.active_sessions || 0) > 0
      },
      { key: 'sync', label: '同步与图片缓存', icon: RefreshCw },
      { key: 'config', label: '系统设置', icon: SlidersHorizontal }
    ]
  }
])

function switchTab(key) {
  if (key !== 'poster') {
    posterInitialCategory.value = ''
    posterInitialStatus.value = ''
  }
  activeTab.value = key
  fetchOverview()
}

function handleDashboardNavigate({ tab, category = '', status = '' }) {
  posterInitialCategory.value = category
  posterInitialStatus.value = status
  activeTab.value = tab
}

async function fetchOverview() {
  refreshingOverview.value = true
  try {
    overview.value = await adminApi.getOverview()
  } catch (_) {
    // 静默忽略首次加载失败
  } finally {
    refreshingOverview.value = false
  }
}

function handleGlobalKeydown(e) {
  if (e.key === 'Escape') {
    if (confirmState.visible) {
      resolveConfirm(false)
    } else if (drawerMediaId.value) {
      drawerMediaId.value = null
    }
  }
}

onMounted(() => {
  fetchOverview()
  window.addEventListener('keydown', handleGlobalKeydown)
})

onUnmounted(() => {
  window.removeEventListener('keydown', handleGlobalKeydown)
})
</script>
