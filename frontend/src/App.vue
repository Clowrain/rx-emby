<script setup>
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import {
  LayoutDashboard,
  Film,
  Sparkles,
  Users,
  RefreshCw,
  SlidersHorizontal,
  Activity,
  HardDrive,
  ShieldCheck,
  LogOut,
  Image as ImageIcon,
  Radio,
  Server,
  Cpu,
  ListTodo
} from 'lucide-vue-next'
import {
  adminApi,
  getAdminToken,
  clearAdminToken,
  setUnauthorizedHandler
} from './api/admin'
import { confirmState, resolveConfirm } from './composables/useToast'
import LoginView from './components/LoginView.vue'
import AdminSidebar from './components/layout/AdminSidebar.vue'
import BuildVersionBadge from './components/layout/BuildVersionBadge.vue'
import ScrapeTaskBanner from './components/layout/ScrapeTaskBanner.vue'
import ToastContainer from './components/layout/ToastContainer.vue'
import ConfirmModal from './components/layout/ConfirmModal.vue'
import OverviewDashboard from './components/OverviewDashboard.vue'
import PosterWallTab from './components/PosterWallTab.vue'
import MediaDetailDrawer from './components/MediaDetailDrawer.vue'
import VirtualLibrariesTab from './components/VirtualLibrariesTab.vue'
import BaseCoversTab from './components/BaseCoversTab.vue'
import ActiveSessionsTab from './components/ActiveSessionsTab.vue'
import UsersAndSessionsTab from './components/UsersAndSessionsTab.vue'
import TaskCenterTab from './components/TaskCenterTab.vue'
import ImageCacheTab from './components/ImageCacheTab.vue'
import HotConfigTab from './components/HotConfigTab.vue'
import ResourceMonitorTab from './components/ResourceMonitorTab.vue'

const authChecking = ref(true)
const currentUser = ref(null)
const authNotice = ref('')
const activeTab = ref('overview')
const drawerMediaId = ref(null)
const overview = ref(null)
const refreshingOverview = ref(false)
const posterInitialCategory = ref('')
const posterInitialStatus = ref('')

const tabMetas = {
  overview: {
    title: '数据总览',
    subtitle: '实时查看媒体库规模、TMDB 元数据刮削进度、当前播放会话与缓存占用'
  },
  poster: {
    title: '媒体库海报墙',
    subtitle: '浏览海报墙、查看多版本片源、手动修正 TMDB 匹配或屏蔽指定条目'
  },
  vlib: {
    title: '智能视图',
    subtitle: '按评分、地区、平台或流派组合规则，在 VidHub / Infuse 客户端首页生成专属分类栏目'
  },
  covers: {
    title: '基础库封面',
    subtitle: '自定义电影、电视剧、动漫、纪录片与综艺的 16:9 客户端首页封面'
  },
  sessions: {
    title: '实时会话',
    subtitle: '监控当前活跃播放流、播放进度心跳与下一集直链预热状态'
  },
  users: {
    title: '用户与设备',
    subtitle: '管理家庭成员账号与分类访问权限，管理客户端已登录设备令牌'
  },
  tasks: {
    title: '任务中心',
    subtitle: '统一管理 TMDB 刮削调度、扩展元数据补齐、媒体库同步进度及历史执行记录'
  },
  cache: {
    title: '图片缓存',
    subtitle: '监控本地磁盘图片缓存水位，一键清理过期缓存并配置空间配额上限'
  },
  resources: {
    title: '资源监控与审计',
    subtitle: '实时观测应用与数据库 CPU、内存、磁盘水位，按日归档峰值场景与日常均值'
  },
  config: {
    title: '系统设置',
    subtitle: '配置同步策略与过滤、小雅 / AList 网关连接与 TMDB 刮削代理等全局核心服务，保存后立即生效'
  }
}

const currentTabMeta = computed(() => tabMetas[activeTab.value] || tabMetas.overview)

watch(
  [currentTabMeta, currentUser],
  ([meta, user]) => {
    document.title = user
      ? `${meta.title} | RX-Emby 管理后台`
      : '登录 | RX-Emby 管理后台'
  },
  { immediate: true }
)

const navGroups = computed(() => [
  {
    title: '媒体内容',
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
      },
      { key: 'covers', label: '基础库封面', icon: ImageIcon }
    ]
  },
  {
    title: '用户与会话',
    items: [
      {
        key: 'sessions',
        label: '实时会话',
        icon: Activity,
        badge: overview.value?.system?.active_sessions || null,
        badgeActive: (overview.value?.system?.active_sessions || 0) > 0
      },
      { key: 'users', label: '用户与设备', icon: Users }
    ]
  },
  {
    title: '运维与设置',
    items: [
      { key: 'tasks', label: '任务中心', icon: ListTodo },
      { key: 'cache', label: '图片缓存', icon: HardDrive },
      { key: 'resources', label: '资源监控', icon: Cpu },
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

async function verifySession() {
  authChecking.value = true
  const token = getAdminToken()
  if (!token) {
    currentUser.value = null
    authChecking.value = false
    return
  }

  try {
    const res = await adminApi.getMe()
    if (res?.user?.is_admin) {
      currentUser.value = res.user
      authNotice.value = ''
      fetchOverview()
    } else {
      clearAdminToken()
      currentUser.value = null
      authNotice.value = '无权访问：仅系统管理员权限角色可登录管理后台'
    }
  } catch (err) {
    clearAdminToken()
    currentUser.value = null
    authNotice.value = err?.message || ''
  } finally {
    authChecking.value = false
  }
}

function handleLoggedIn(user) {
  currentUser.value = user
  authNotice.value = ''
  activeTab.value = 'overview'
  fetchOverview()
}

async function handleLogout() {
  try {
    await adminApi.logout()
  } catch (_) {
    // 忽略退出请求异常，本地仍清理凭证
  } finally {
    clearAdminToken()
    currentUser.value = null
    overview.value = null
    drawerMediaId.value = null
    authNotice.value = ''
  }
}

onMounted(() => {
  setUnauthorizedHandler(msg => {
    currentUser.value = null
    overview.value = null
    drawerMediaId.value = null
    authNotice.value = msg || '登录已失效，请重新登录'
  })
  verifySession()
  window.addEventListener('keydown', handleGlobalKeydown)
})

onUnmounted(() => {
  window.removeEventListener('keydown', handleGlobalKeydown)
})
</script>

<template>
  <!-- 1. 鉴权检查中 -->
  <div v-if="authChecking" class="min-h-dvh bg-[#060911] text-slate-300 flex items-center justify-center">
    <div class="flex flex-col items-center gap-3">
      <div class="w-9 h-9 rounded-xl border-2 border-emerald-500/30 border-t-emerald-400 animate-spin"></div>
      <p class="text-xs text-slate-400">正在校验管理后台会话…</p>
    </div>
  </div>

  <!-- 2. 未登录或非系统管理员 -->
  <LoginView
    v-else-if="!currentUser"
    :notice="authNotice"
    @logged-in="handleLoggedIn"
  />

  <!-- 3. 已登录系统管理员 -->
  <div v-else class="min-h-dvh flex bg-[#060911] text-slate-100">
    <a href="#main-workspace" class="skip-link visually-hidden">跳转到主工作区</a>

    <AdminSidebar
      :nav-groups="navGroups"
      :active-tab="activeTab"
      @switch-tab="switchTab"
    />

    <!-- 右侧主工作区 -->
    <div class="flex-1 flex flex-col min-w-0">
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
            @click="switchTab('sessions')"
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
            @click="switchTab('cache')"
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

          <div class="flex items-center gap-2 pl-2 border-l border-slate-800/80">
            <span class="px-2.5 py-1.5 rounded-xl bg-emerald-500/10 border border-emerald-500/25 text-xs text-emerald-300 font-medium flex items-center gap-1.5">
              <ShieldCheck class="w-3.5 h-3.5 text-emerald-400" aria-hidden="true" />
              <span>{{ currentUser.username }}</span>
            </span>
            <button
              type="button"
              @click="handleLogout"
              class="cursor-pointer px-2.5 py-1.5 rounded-xl bg-slate-900/90 hover:bg-rose-950/70 border border-slate-800 hover:border-rose-500/40 text-xs font-medium text-slate-300 hover:text-rose-200 transition-colors flex items-center gap-1.5"
              title="退出登录"
            >
              <LogOut class="w-3.5 h-3.5" aria-hidden="true" />
              <span>退出</span>
            </button>
          </div>
        </div>
      </header>

      <ScrapeTaskBanner @completed="fetchOverview" @open-tasks="switchTab('tasks')" />

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
        <BaseCoversTab v-else-if="activeTab === 'covers'" />
        <ActiveSessionsTab v-else-if="activeTab === 'sessions'" />
        <UsersAndSessionsTab v-else-if="activeTab === 'users'" />
        <TaskCenterTab v-else-if="activeTab === 'tasks'" />
        <ImageCacheTab v-else-if="activeTab === 'cache'" @saved="fetchOverview" />
        <ResourceMonitorTab v-else-if="activeTab === 'resources'" />
        <HotConfigTab v-else-if="activeTab === 'config'" @saved="fetchOverview" />
      </main>

      <footer class="border-t border-slate-800/70 bg-[#080c16]/60 backdrop-blur-sm">
        <div class="max-w-[1600px] w-full mx-auto px-8 py-3.5 flex flex-wrap items-center justify-between gap-x-6 gap-y-2.5">
          <!-- 左侧：302 直链转发引擎运行态 -->
          <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-xs font-mono tabular-nums text-slate-400 min-w-0">
            <span class="inline-flex items-center gap-2 text-slate-200 font-sans font-medium">
              <Radio class="w-3.5 h-3.5 text-emerald-400 animate-pulse shrink-0" aria-hidden="true" />
              <span>302 直链转发引擎</span>
              <span class="text-[10px] font-mono font-semibold px-1.5 py-0.5 rounded bg-emerald-500/15 text-emerald-300 border border-emerald-500/25">
                运行中
              </span>
            </span>

            <span class="text-slate-600" aria-hidden="true">·</span>

            <span
              class="inline-flex items-center gap-1.5 max-w-[260px] truncate text-slate-300"
              :title="overview?.system?.alist_base_url || '正在连接网关'"
            >
              <Server class="w-3.5 h-3.5 text-slate-400 shrink-0" aria-hidden="true" />
              <span class="truncate">{{ overview?.system?.alist_base_url || '正在连接网关…' }}</span>
            </span>

            <span class="text-slate-600" aria-hidden="true">·</span>

            <span class="text-slate-400">
              TMDB {{ overview?.system?.tmdb_rate_limit || 8 }} req/s
            </span>

            <span class="text-slate-600" aria-hidden="true">·</span>

            <span class="text-slate-300 font-sans">
              {{ overview?.system?.auto_background_scraper ? '自动刮削' : '按需刮削' }}
            </span>
          </div>

          <!-- 右侧：版本号、GitHub Commit 链接与构建时间 -->
          <BuildVersionBadge variant="footer" />
        </div>
      </footer>
    </div>

    <MediaDetailDrawer :media-id="drawerMediaId" @close="drawerMediaId = null" />
    <ToastContainer />
    <ConfirmModal />
  </div>
</template>
