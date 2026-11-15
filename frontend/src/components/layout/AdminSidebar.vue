<script setup>
import { Play } from 'lucide-vue-next'

defineProps({
  navGroups: { type: Array, required: true },
  activeTab: { type: String, required: true }
})

const emit = defineEmits(['switch-tab'])
</script>

<template>
  <aside class="w-64 shrink-0 bg-[#080c16]/95 border-r border-slate-800/75 flex flex-col sticky top-0 h-dvh z-30">
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
      <nav aria-label="主导航" class="p-3.5 space-y-4 overflow-y-auto max-h-[calc(100dvh-88px)]">
        <div v-for="group in navGroups" :key="group.title" class="space-y-1">
          <div class="px-3 text-[11px] font-semibold tracking-wide text-slate-500 mb-1.5">
            {{ group.title }}
          </div>
          <button
            v-for="item in group.items"
            :key="item.key"
            type="button"
            @click="emit('switch-tab', item.key)"
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
  </aside>
</template>
