<template>
  <section class="space-y-6">
    <div class="card p-4 rounded-2xl flex flex-wrap gap-2 items-center">
      <div role="tablist" aria-label="系统核心设置分组" class="flex flex-wrap gap-2">
        <button
          v-for="sec in sections"
          :key="sec.key"
          type="button"
          role="tab"
          :aria-selected="activeSection === sec.key"
          @click="activeSection = sec.key"
          class="cursor-pointer px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-2"
          :class="
            activeSection === sec.key
              ? 'bg-emerald-600 text-white shadow-sm'
              : 'bg-slate-900/90 text-slate-400 hover:text-slate-200 border border-slate-800'
          "
        >
          <component :is="sec.icon" class="w-3.5 h-3.5 shrink-0" :stroke-width="1.85" aria-hidden="true" />
          <span>{{ sec.title }}</span>
        </button>
      </div>
    </div>

    <ConfigGroupPanel
      :key="currentSectionMeta.key"
      :prefix="currentSectionMeta.prefix"
      :title="currentSectionMeta.title"
      :description="currentSectionMeta.desc"
      :icon="currentSectionMeta.icon"
      @saved="$emit('saved')"
    />
  </section>
</template>

<script setup>
import { ref, computed } from 'vue'
import { SlidersHorizontal, Link2, Crosshair } from 'lucide-vue-next'
import ConfigGroupPanel from './ConfigGroupPanel.vue'

defineEmits(['saved'])

const activeSection = ref('sync')

const sections = [
  {
    key: 'sync',
    icon: SlidersHorizontal,
    title: '同步策略与过滤',
    prefix: 'sync.',
    desc: '配置 strm.txt 索引源路径、定时同步周期、目录白名单前缀、排除关键词与调试行数上限。'
  },
  {
    key: 'alist',
    icon: Link2,
    title: '小雅 / AList 网关与直链',
    prefix: 'alist.',
    desc: '配置内网小雅或 AList 服务地址、自动登录凭据以及 302 网盘直链内存缓存有效期。'
  },
  {
    key: 'tmdb',
    icon: Crosshair,
    title: 'TMDB 刮削与网络代理',
    prefix: 'tmdb.|scraper.',
    desc: '配置 TMDB API 密钥、HTTP/SOCKS5 代理、自定义镜像域名、令牌桶限流速率及后台自动刮削开关。'
  }
]

const currentSectionMeta = computed(
  () => sections.find(s => s.key === activeSection.value) || sections[0]
)
</script>
