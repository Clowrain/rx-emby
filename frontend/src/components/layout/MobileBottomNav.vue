<script setup>
import { computed } from 'vue'
import { LayoutDashboard, Film, ListTodo, Cpu, Menu } from 'lucide-vue-next'

const props = defineProps({
  activeTab: { type: String, required: true },
  menuOpen: { type: Boolean, default: false }
})

const emit = defineEmits(['switch-tab', 'open-menu'])

const shortcuts = [
  { key: 'overview', label: '总览', icon: LayoutDashboard },
  { key: 'poster', label: '海报墙', icon: Film },
  { key: 'tasks', label: '任务', icon: ListTodo },
  { key: 'resources', label: '监控', icon: Cpu }
]

const shortcutKeys = new Set(shortcuts.map((s) => s.key))
const isMoreActive = computed(() => props.menuOpen || !shortcutKeys.has(props.activeTab))
</script>

<template>
  <nav
    aria-label="移动端快捷导航"
    class="lg:hidden fixed bottom-0 inset-x-0 z-30 bg-[#080c16]/95 backdrop-blur-md border-t border-slate-800/85 pb-[env(safe-area-inset-bottom)]"
  >
    <div class="grid grid-cols-5 items-stretch">
      <button
        v-for="item in shortcuts"
        :key="item.key"
        type="button"
        @click="emit('switch-tab', item.key)"
        :aria-current="activeTab === item.key && !menuOpen ? 'page' : undefined"
        class="cursor-pointer flex flex-col items-center justify-center gap-1 min-h-[54px] px-1 py-1.5 text-[11px] font-medium transition-colors active:scale-95"
        :class="
          activeTab === item.key && !menuOpen
            ? 'text-emerald-400'
            : 'text-slate-400 hover:text-slate-200'
        "
      >
        <component
          :is="item.icon"
          class="w-5 h-5 shrink-0"
          :stroke-width="activeTab === item.key && !menuOpen ? 2.2 : 1.8"
          aria-hidden="true"
        />
        <span class="truncate max-w-full">{{ item.label }}</span>
      </button>

      <button
        type="button"
        @click="emit('open-menu')"
        :aria-expanded="menuOpen"
        class="cursor-pointer flex flex-col items-center justify-center gap-1 min-h-[54px] px-1 py-1.5 text-[11px] font-medium transition-colors active:scale-95"
        :class="isMoreActive ? 'text-emerald-400' : 'text-slate-400 hover:text-slate-200'"
      >
        <Menu class="w-5 h-5 shrink-0" :stroke-width="isMoreActive ? 2.2 : 1.8" aria-hidden="true" />
        <span>更多</span>
      </button>
    </div>
  </nav>
</template>
