<script setup lang="ts">
import { onMounted, onUnmounted } from 'vue'
import { Sparkles, Plus, Film, Tv, SlidersHorizontal, Trash2, Pencil, Power, RefreshCw } from 'lucide-vue-next'
import VirtualLibraryFormModal from './virtual-libraries/VirtualLibraryFormModal.vue'
import { useVirtualLibraries } from '../composables/useVirtualLibraries.js'
import {
  coverImageUrl,
  dynamicCoverLabel,
  isDynamicCover,
  ruleBadges
} from '../utils/smartViewLabels.js'

const emit = defineEmits<{
  openTasks: []
}>()

const {
  loading,
  saving,
  seeding,
  backfilling,
  refreshingCovers,
  previewing,
  togglingId,
  deletingId,
  listFilter,
  listQuery,
  previewCount,
  formError,
  optionsLoaded,
  ruleOptions,
  libraries,
  coverBust,
  modal,
  filterTabs,
  filteredLibraries,
  metadataPercent,
  needsMetadataBackfill,
  loadLibraries,
  loadRuleOptions,
  handleRefreshDynamicCovers,
  openModal,
  openInfoModal,
  openRulesModal,
  openCreateModal,
  nextCreateStep,
  previewRules,
  submitModal,
  backfillMetadata,
  handleSeedPresets,
  toggleEnabled,
  handleDelete
} = useVirtualLibraries()

function handleScrapeCompleted() {
  loadLibraries()
  loadRuleOptions()
}

onMounted(() => {
  loadLibraries()
  loadRuleOptions()
  if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
    window.addEventListener('rx-scrape-task-completed', handleScrapeCompleted)
  }
})

onUnmounted(() => {
  if (typeof window !== 'undefined' && typeof window.removeEventListener === 'function') {
    window.removeEventListener('rx-scrape-task-completed', handleScrapeCompleted)
  }
})
</script>

<template>
  <section class="space-y-6">
    <div class="card p-6 rounded-2xl flex flex-wrap gap-4 items-center justify-between">
      <div>
        <h2 class="text-base font-bold text-white flex items-center gap-2">
          <Sparkles class="w-4 h-4 text-emerald-400" aria-hidden="true" />
          <span>智能视图管理</span>
        </h2>
        <p class="text-xs text-slate-400 mt-1">
          按地区、平台、流派和评分组合筛选规则，在 VidHub / Infuse 客户端首页生成专属分类栏目。
        </p>
      </div>
      <div class="flex gap-2.5">
        <button
          type="button"
          @click="handleSeedPresets"
          :disabled="seeding"
          class="cursor-pointer bg-indigo-600/25 hover:bg-indigo-600/40 border border-indigo-500/40 text-indigo-200 px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
        >
          <Sparkles class="w-3.5 h-3.5 text-indigo-300" aria-hidden="true" />
          <span>{{ seeding ? '正在生成…' : '补充 9 个预设视图' }}</span>
        </button>
        <button
          type="button"
          @click="openCreateModal()"
          :disabled="saving"
          class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 text-white px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
        >
          <Plus class="w-3.5 h-3.5" aria-hidden="true" />
          <span>新建智能视图</span>
        </button>
      </div>
    </div>

    <div v-if="loading" class="text-center py-12 text-slate-400 text-xs">正在加载智能视图…</div>
    <div
      v-else-if="libraries.length === 0"
      class="card p-12 rounded-2xl text-center space-y-3"
    >
      <div class="w-12 h-12 rounded-2xl bg-indigo-500/15 border border-indigo-500/30 text-indigo-300 flex items-center justify-center mx-auto">
        <Sparkles class="w-6 h-6" :stroke-width="1.75" aria-hidden="true" />
      </div>
      <div class="text-sm font-medium text-slate-200">尚未配置智能视图</div>
      <div class="text-xs text-slate-400">
        点击右上角补充预设，快速创建高分佳作、已完结短剧和 4K 杜比视界等栏目
      </div>
    </div>
    <div v-else class="space-y-4">
      <!-- 列表筛选与搜索工具栏 -->
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div class="flex flex-wrap items-center gap-1.5">
          <button
            v-for="tab in filterTabs"
            :key="tab.key"
            type="button"
            @click="listFilter = tab.key"
            class="cursor-pointer px-3 py-1.5 rounded-xl text-xs font-medium transition-colors border tabular-nums"
            :class="
              listFilter === tab.key
                ? 'bg-emerald-600/20 border-emerald-500/40 text-emerald-200'
                : 'bg-slate-900/80 border-slate-800 text-slate-400 hover:text-slate-200 hover:border-slate-700'
            "
          >
            {{ tab.label }}（{{ tab.count }}）
          </button>
        </div>
        <div class="flex items-center gap-2 w-full sm:w-auto">
          <button
            type="button"
            :disabled="refreshingCovers"
            @click="handleRefreshDynamicCovers"
            class="cursor-pointer disabled:opacity-50 px-3 py-1.5 rounded-xl bg-slate-900/90 hover:bg-slate-800 border border-slate-800 text-xs font-medium text-slate-200 flex items-center gap-1.5 shrink-0 transition-colors"
            title="清理 1 天动态封面缓存，立即按最新发布时间重新生成预览"
          >
            <RefreshCw class="w-3.5 h-3.5 text-emerald-400" :class="refreshingCovers ? 'animate-spin' : ''" aria-hidden="true" />
            <span>{{ refreshingCovers ? '刷新中…' : '刷新动态封面' }}</span>
          </button>
          <div class="relative flex-1 sm:w-64">
            <input
              v-model="listQuery"
              type="search"
              aria-label="搜索智能视图"
              placeholder="搜索视图名称或规则…"
              class="w-full px-3.5 py-1.5 rounded-xl bg-slate-900/90 border border-slate-800 focus:border-emerald-500 text-xs text-slate-200 placeholder:text-slate-500 focus:outline-none"
            />
          </div>
        </div>
      </div>

      <div
        v-if="filteredLibraries.length === 0"
        class="card p-10 rounded-2xl text-center space-y-2.5"
      >
        <p class="text-sm font-medium text-slate-300">没有符合筛选条件的智能视图</p>
        <button
          type="button"
          @click="listQuery = ''; listFilter = 'all'"
          class="cursor-pointer px-3.5 py-1.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-xs font-medium text-emerald-300 transition-colors"
        >
          重置筛选
        </button>
      </div>

      <div v-else class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
        <div
          v-for="v in filteredLibraries"
          :key="v.id"
          class="card card-interactive rounded-2xl overflow-hidden flex flex-col justify-between"
          :class="!v.is_enabled ? 'border-slate-800/60' : ''"
        >
          <div>
            <div class="relative aspect-video bg-slate-950 border-b border-slate-800/80 overflow-hidden group">
              <button
                type="button"
                @click="openInfoModal(v)"
                :aria-label="`设置智能视图 ${v.name} 名称与封面`"
                title="点击设置名称与封面"
                class="cursor-pointer block w-full h-full text-left"
              >
                <img
                  :src="coverImageUrl(v.view_id, v.cover_image, v.resolved_cover || 'vlib_default_tv', coverBust)"
                  :alt="v.name"
                  class="w-full h-full object-cover transition-all duration-300 group-hover:scale-105"
                  :class="!v.is_enabled ? 'opacity-60 grayscale-[30%]' : ''"
                />
              </button>
              <button
                type="button"
                @click.stop="toggleEnabled(v)"
                :disabled="togglingId === v.id"
                :title="v.is_enabled ? '点击停用（从客户端首页移除）' : '点击启用（挂载至客户端首页）'"
                :aria-label="v.is_enabled ? `停用 ${v.name}` : `启用 ${v.name}`"
                class="cursor-pointer absolute top-2 right-2 px-2 py-0.5 text-[11px] font-medium rounded-md backdrop-blur-xs shadow-sm transition-colors inline-flex items-center gap-1 disabled:opacity-50"
                :class="
                  v.is_enabled
                    ? 'bg-emerald-950/90 hover:bg-emerald-900 text-emerald-300 border border-emerald-500/40'
                    : 'bg-slate-900/90 hover:bg-slate-800 text-slate-300 border border-slate-700/70'
                "
              >
                <Power class="w-3 h-3 shrink-0" aria-hidden="true" />
                <span>{{ togglingId === v.id ? '切换中…' : (v.is_enabled ? '首页已挂载' : '已停用') }}</span>
              </button>
              <div
                v-if="isDynamicCover(v.cover_image)"
                class="pointer-events-none absolute bottom-2 left-2 px-2 py-0.5 rounded-md bg-slate-950/85 backdrop-blur-xs border border-white/10 text-[11px] font-medium text-amber-300"
              >
                {{ dynamicCoverLabel(v.cover_image) }}
              </div>
            </div>

            <div class="px-3.5 py-3">
              <div class="flex items-center justify-between gap-2 min-w-0">
                <div class="flex items-center gap-2 min-w-0">
                  <span class="w-7 h-7 rounded-lg bg-slate-900 border border-slate-700/80 text-emerald-400 flex items-center justify-center shrink-0">
                    <Film v-if="v.icon_type === 'movies'" class="w-3.5 h-3.5" :stroke-width="1.85" aria-hidden="true" />
                    <Tv v-else class="w-3.5 h-3.5" :stroke-width="1.85" aria-hidden="true" />
                  </span>
                  <div class="min-w-0">
                    <div class="font-bold text-[13px] text-white truncate" :title="v.name">{{ v.name }}</div>
                    <div
                      class="text-[11px] text-slate-400 tabular-nums truncate"
                      :title="ruleBadges(v.filter_rules, ruleOptions).join(' · ') || '无附加过滤条件（匹配全库）'"
                    >
                      排序 #{{ v.sort_order }} · {{ ruleBadges(v.filter_rules, ruleOptions).length ? `${ruleBadges(v.filter_rules, ruleOptions).length} 项规则` : '匹配全库' }}
                    </div>
                  </div>
                </div>
                <div
                  class="shrink-0 px-2 py-0.5 rounded-lg border text-[11px] font-mono font-bold tabular-nums"
                  :class="
                    Number(v.matched_count || 0) === 0
                      ? 'bg-amber-950/25 border-amber-500/30 text-amber-300'
                      : 'bg-slate-950/80 border-slate-800/80 text-emerald-400'
                  "
                  :title="Number(v.matched_count || 0) === 0 ? '当前暂无匹配影视，请检查筛选规则或先补齐扩展信息' : undefined"
                >
                  {{ Number(v.matched_count || 0).toLocaleString() }} 部
                </div>
              </div>
            </div>
          </div>

          <div class="flex gap-2 px-3.5 pb-3 pt-2.5 border-t border-slate-800/70">
            <button
              type="button"
              @click="openInfoModal(v)"
              :disabled="saving"
              title="设置名称与封面"
              :aria-label="`设置智能视图 ${v.name} 名称与封面`"
              class="cursor-pointer px-2.5 py-1.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-300 hover:text-emerald-300 transition-colors flex items-center justify-center disabled:opacity-50"
            >
              <Pencil class="w-3.5 h-3.5" aria-hidden="true" />
            </button>
            <button
              type="button"
              @click="openRulesModal(v)"
              :disabled="saving"
              :aria-label="`编辑智能视图 ${v.name} 筛选规则`"
              class="cursor-pointer flex-1 py-1.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-xs font-medium text-slate-200 transition-colors flex items-center justify-center gap-1.5 disabled:opacity-50"
            >
              <SlidersHorizontal class="w-3.5 h-3.5 text-slate-400" aria-hidden="true" />
              <span>编辑规则</span>
            </button>
            <button
              type="button"
              @click="handleDelete(v)"
              :disabled="deletingId === v.id || saving"
              title="删除智能视图"
              :aria-label="`删除智能视图 ${v.name}`"
              class="cursor-pointer px-2.5 py-1.5 rounded-xl bg-rose-950/60 hover:bg-rose-900/80 border border-rose-800/50 text-rose-300 transition-colors flex items-center justify-center disabled:opacity-50"
            >
              <Trash2 class="w-3.5 h-3.5" aria-hidden="true" />
            </button>
          </div>
        </div>
      </div>
    </div>

    <VirtualLibraryFormModal
      :modal="modal"
      :rule-options="ruleOptions"
      :needs-metadata-backfill="needsMetadataBackfill"
      :backfilling="backfilling"
      :saving="saving"
      :previewing="previewing"
      :preview-count="previewCount"
      :form-error="formError"
      @close="modal.visible = false"
      @submit="submitModal"
      @next-step="nextCreateStep"
      @prev-step="formError = ''; modal.step = 1"
      @preview="previewRules"
      @backfill="backfillMetadata"
      @open-tasks="modal.visible = false; emit('openTasks')"
    />
  </section>
</template>
