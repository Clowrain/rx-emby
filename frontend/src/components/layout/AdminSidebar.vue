<script setup>
import { Play, X, Activity, HardDrive, ShieldCheck, LogOut } from 'lucide-vue-next'
import BuildVersionBadge from './BuildVersionBadge.vue'

defineProps({
  navGroups: { type: Array, required: true },
  activeTab: { type: String, required: true },
  mobileOpen: { type: Boolean, default: false },
  currentUser: { type: Object, default: null },
  overview: { type: Object, default: null }
})

const emit = defineEmits(['switch-tab', 'close', 'logout'])

function handleSelectTab(key) {
  emit('switch-tab', key)
  emit('close')
}
</script>

<template>
  <!-- 移动端遮罩 -->
  <div
    v-if="mobileOpen"
    class="lg:hidden fixed inset-0 bg-black/70 backdrop-blur-xs z-40"
    aria-hidden="true"
    @click="emit('close')"
  ></div>

  <!-- 桌面端固定侧边栏 + 移动端滑出抽屉 -->
  <aside
    aria-label="导航菜单"
    class="w-72 lg:w-64 shrink-0 bg-[#080c16]/98 lg:bg-[#080c16]/95 border-r border-slate-800/75 flex flex-col fixed lg:sticky inset-y-0 left-0 h-dvh z-50 lg:z-30 transition-transform duration-200 ease-out overscroll-contain pt-[env(safe-area-inset-top)] lg:pt-0 pb-[env(safe-area-inset-bottom)] lg:pb-0"
    :class="mobileOpen ? 'translate-x-0 shadow-2xl' : '-translate-x-full lg:translate-x-0'"
  >
    <!-- 品牌 Logo 区 -->
    <div class="px-5 lg:px-6 py-4 lg:py-5 border-b border-slate-800/70 flex items-center justify-between gap-3 shrink-0">
      <div class="flex items-center gap-3.5 min-w-0">
        <div class="w-9 h-9 rounded-xl bg-gradient-to-br from-emerald-500 to-cyan-600 flex items-center justify-center shadow-lg shadow-emerald-950/80 ring-1 ring-white/15 shrink-0">
          <Play class="w-4 h-4 text-white fill-current ml-0.5" aria-hidden="true" />
        </div>
        <div class="min-w-0">
          <div class="flex items-center gap-2">
            <span class="font-bold text-base tracking-tight text-white">RX-Emby</span>
            <span class="px-1.5 py-0.5 text-[10px] font-semibold font-mono rounded bg-emerald-500/15 text-emerald-400 border border-emerald-500/30">
              PRO
            </span>
          </div>
          <p class="text-[11px] text-slate-400 mt-0.5 truncate">零落盘直链流媒体服务</p>
        </div>
      </div>

      <button
        type="button"
        @click="emit('close')"
        class="lg:hidden cursor-pointer w-10 h-10 rounded-xl bg-slate-900/90 border border-slate-800 text-slate-400 hover:text-white flex items-center justify-center shrink-0"
        aria-label="关闭导航菜单"
      >
        <X class="w-5 h-5" aria-hidden="true" />
      </button>
    </div>

    <!-- 移动端实时运行态摘要 -->
    <div class="lg:hidden px-4 py-3 border-b border-slate-800/60 grid grid-cols-2 gap-2 shrink-0">
      <button
        type="button"
        @click="handleSelectTab('sessions')"
        class="cursor-pointer flex items-center gap-2 px-3 py-2 rounded-xl bg-slate-900/90 border border-slate-800 text-xs text-left tabular-nums"
      >
        <Activity
          class="w-3.5 h-3.5 shrink-0"
          :class="(overview?.system?.active_sessions || 0) > 0 ? 'text-emerald-400 animate-pulse' : 'text-slate-500'"
          aria-hidden="true"
        />
        <div class="min-w-0">
          <div class="text-[10px] text-slate-400">并发播放流</div>
          <div class="font-semibold font-mono text-white">{{ overview?.system?.active_sessions || 0 }}</div>
        </div>
      </button>

      <button
        type="button"
        @click="handleSelectTab('cache')"
        class="cursor-pointer flex items-center gap-2 px-3 py-2 rounded-xl bg-slate-900/90 border border-slate-800 text-xs text-left tabular-nums"
      >
        <HardDrive class="w-3.5 h-3.5 text-cyan-400 shrink-0" aria-hidden="true" />
        <div class="min-w-0">
          <div class="text-[10px] text-slate-400">图片缓存</div>
          <div class="font-mono text-emerald-400 truncate">
            {{ overview?.cache?.used_mb || 0 }} MB
          </div>
        </div>
      </button>
    </div>

    <!-- 分组菜单 -->
    <nav aria-label="主导航" class="flex-1 p-3.5 space-y-4 overflow-y-auto overscroll-contain">
      <div v-for="group in navGroups" :key="group.title" class="space-y-1">
        <div class="px-3 text-[11px] font-semibold tracking-wide text-slate-500 mb-1.5">
          {{ group.title }}
        </div>
        <button
          v-for="item in group.items"
          :key="item.key"
          type="button"
          @click="handleSelectTab(item.key)"
          :aria-current="activeTab === item.key ? 'page' : undefined"
          class="w-full flex items-center justify-between px-3 py-2.5 min-h-[44px] rounded-xl text-sm font-medium transition-colors duration-150 cursor-pointer"
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

    <!-- 移动端抽屉底部：账号与退出 -->
    <div v-if="currentUser" class="lg:hidden px-4 py-3 border-t border-slate-800/70 space-y-2.5 shrink-0 bg-[#060911]/60">
      <div class="flex items-center justify-between gap-2">
        <span class="px-2.5 py-1.5 rounded-xl bg-emerald-500/10 border border-emerald-500/25 text-xs text-emerald-300 font-medium flex items-center gap-1.5 min-w-0">
          <ShieldCheck class="w-3.5 h-3.5 text-emerald-400 shrink-0" aria-hidden="true" />
          <span class="truncate">{{ currentUser.username }}</span>
        </span>
        <button
          type="button"
          @click="emit('logout')"
          class="cursor-pointer min-h-[40px] px-3 py-1.5 rounded-xl bg-slate-900/90 hover:bg-rose-950/70 border border-slate-800 hover:border-rose-500/40 text-xs font-medium text-slate-300 hover:text-rose-200 transition-colors flex items-center gap-1.5 shrink-0"
        >
          <LogOut class="w-3.5 h-3.5" aria-hidden="true" />
          <span>退出登录</span>
        </button>
      </div>
      <BuildVersionBadge variant="footer" />
    </div>
  </aside>
</template>
