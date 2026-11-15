<script setup>
import { Film, Layers, Star, Link2, Sparkles, Eye, EyeOff } from 'lucide-vue-next'

const props = defineProps({
  item: { type: Object, required: true },
  scraping: { type: Boolean, default: false }
})

const emit = defineEmits(['open-detail', 'open-bind', 'scrape', 'toggle-ignore'])

function displayTitle(item) {
  return item.title || `作品 #${item.id}`
}

function statusBadgeText(item) {
  return item.is_locked ? '已锁定' : '已入库'
}

function statusBadgeClass(item) {
  return item.is_locked
    ? 'bg-amber-950/90 text-amber-300 border border-amber-500/40'
    : 'bg-emerald-950/90 text-emerald-300 border border-emerald-500/40'
}
</script>

<template>
  <div class="card card-interactive rounded-2xl overflow-hidden flex flex-col justify-between group">
    <!-- 海报主体与标题信息区 -->
    <button
      type="button"
      @click="emit('open-detail', item.id)"
      class="cursor-pointer text-left w-full block focus-visible:outline-offset-[-2px]"
    >
      <div class="aspect-[2/3] bg-slate-950 relative overflow-hidden flex items-center justify-center">
        <img
          v-if="item.poster_path"
          :src="`/emby/Items/work_${item.id}/Images/Primary`"
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

        <div class="absolute inset-x-2 top-2 flex items-center justify-between gap-1.5 pointer-events-none">
          <div class="flex items-center gap-1 min-w-0">
            <span
              class="px-2 py-0.5 text-[10px] font-medium rounded-md backdrop-blur-md shadow whitespace-nowrap shrink-0"
              :class="statusBadgeClass(item)"
            >
              {{ statusBadgeText(item) }}
            </span>
            <span
              v-if="item.folder_count > 1"
              class="px-1.5 py-0.5 text-[10px] font-medium font-mono rounded-md bg-purple-950/90 text-purple-200 border border-purple-500/40 backdrop-blur-md whitespace-nowrap tabular-nums shrink-0"
            >
              ×{{ item.folder_count }} 目录
            </span>
          </div>

          <span class="px-2 py-0.5 text-[10px] rounded-md bg-slate-950/85 text-slate-200 backdrop-blur-md border border-slate-700/60 whitespace-nowrap shrink-0">
            {{ item.category }}
          </span>
        </div>

        <div class="absolute inset-0 bg-slate-950/45 opacity-0 group-hover:opacity-100 transition-opacity duration-200 flex items-center justify-center pointer-events-none">
          <span class="px-3 py-1.5 rounded-full bg-slate-950/90 border border-emerald-500/40 text-[11px] font-medium text-emerald-300 shadow-lg flex items-center gap-1.5">
            <Layers class="w-3.5 h-3.5" aria-hidden="true" />
            <span>查看源明细</span>
          </span>
        </div>

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

      <div class="px-3 pt-2.5 pb-2 space-y-1">
        <div class="flex items-baseline justify-between gap-1.5 min-w-0">
          <span class="font-semibold text-xs text-slate-100 truncate group-hover:text-emerald-400 transition-colors">
            {{ displayTitle(item) }}
          </span>
          <span v-if="item.year" class="text-[11px] text-slate-500 font-mono tabular-nums shrink-0">
            {{ item.year }}
          </span>
        </div>
        <div class="single-line-reveal text-[11px] text-slate-500 font-mono">
          <span>{{ item.media_type === 'movie' ? '电影' : '剧集' }} · {{ item.folder_count || 1 }} 个物理目录</span>
        </div>
      </div>
    </button>

    <!-- 卡片底部工具栏 -->
    <div class="px-2.5 py-2 border-t border-slate-800/60 bg-slate-950/40 flex items-center justify-between gap-1.5">
      <button
        type="button"
        @click="emit('open-bind', item)"
        :aria-label="item.tmdb_id ? `修改 ${displayTitle(item)} 的 TMDB ID (当前 #${item.tmdb_id})` : `为 ${displayTitle(item)} 手动绑定 TMDB ID`"
        class="cursor-pointer min-w-0 flex-1 h-7 px-2 rounded-lg bg-slate-900/90 hover:bg-slate-800 border border-slate-800/90 hover:border-slate-700 text-[11px] text-slate-300 hover:text-white font-mono transition-colors flex items-center gap-1.5 tabular-nums whitespace-nowrap overflow-hidden"
      >
        <Link2 class="w-3 h-3 text-emerald-400 shrink-0" aria-hidden="true" />
        <span class="truncate">{{ item.tmdb_id ? `#${item.tmdb_id}` : '绑定 TMDB' }}</span>
      </button>

      <div class="flex items-center gap-1 shrink-0">
        <button
          type="button"
          @click="emit('scrape', item)"
          :disabled="scraping"
          :aria-label="`重新刮削 ${displayTitle(item)}`"
          class="cursor-pointer h-7 px-2.5 rounded-lg bg-emerald-600/20 hover:bg-emerald-600/35 border border-emerald-500/30 text-emerald-300 text-[11px] font-medium transition-colors disabled:opacity-50 flex items-center gap-1 whitespace-nowrap shrink-0"
        >
          <Sparkles class="w-3 h-3 shrink-0" :class="{ 'animate-spin': scraping }" aria-hidden="true" />
          <span>{{ scraping ? '刮削中' : '刮削' }}</span>
        </button>

        <button
          type="button"
          @click="emit('toggle-ignore', item)"
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
</template>
