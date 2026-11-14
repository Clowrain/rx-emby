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
          @click="openCreateModal()" :disabled="saving"
          class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 text-white px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
        >
          <Plus class="w-3.5 h-3.5" aria-hidden="true" />
          <span>新建智能视图</span>
        </button>
      </div>
    </div>

    <div
      v-if="metadataPercent < 100 || (ruleOptions.metadata?.queued || 0) > 0 || (ruleOptions.metadata?.failed || 0) > 0"
      class="card p-5 rounded-2xl space-y-3"
    >
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div class="space-y-1">
          <div class="flex items-center gap-2.5 flex-wrap">
            <p class="text-sm font-semibold text-slate-200 tabular-nums">
              TMDB 扩展信息已补齐 {{ (ruleOptions.metadata?.enriched || 0).toLocaleString() }} / {{ (ruleOptions.metadata?.total || 0).toLocaleString() }} 部
            </p>
            <span class="px-2 py-0.5 rounded-md bg-slate-800/90 border border-slate-700/80 text-[11px] font-mono text-emerald-300 tabular-nums">
              {{ metadataPercent }}%
            </span>
          </div>
          <p class="text-slate-400 text-xs">
            未补齐语言、时长、投票数等信息的影视暂不参与对应高级规则匹配。队列中 {{ ruleOptions.metadata?.queued || 0 }} 部，失败 {{ ruleOptions.metadata?.failed || 0 }} 部。
          </p>
        </div>
        <div class="flex items-center gap-2">
          <button
            type="button"
            @click="loadRuleOptions"
            class="cursor-pointer px-3.5 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 border border-slate-700/80 text-xs font-medium text-slate-200 transition-colors"
          >
            刷新进度
          </button>
          <button
            type="button"
            @click="backfillMetadata"
            :disabled="backfilling"
            class="cursor-pointer px-4 py-2 rounded-xl bg-emerald-600 hover:bg-emerald-500 text-xs font-semibold text-white transition-colors disabled:opacity-50"
          >
            {{ backfilling ? '正在加入队列…' : '从本地刮削补齐' }}
          </button>
        </div>
      </div>
      <div class="h-1.5 rounded-full bg-slate-800/90 overflow-hidden" role="progressbar" :aria-valuenow="metadataPercent" aria-valuemin="0" aria-valuemax="100" aria-label="TMDB 扩展信息补齐进度">
        <div class="h-full bg-emerald-500 transition-all duration-300" :style="{ width: `${metadataPercent}%` }" />
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
        <div class="relative w-full sm:w-64">
          <input
            v-model="listQuery"
            type="search"
            aria-label="搜索智能视图"
            placeholder="搜索视图名称或规则…"
            class="w-full px-3.5 py-1.5 rounded-xl bg-slate-900/90 border border-slate-800 focus:border-emerald-500 text-xs text-slate-200 placeholder:text-slate-500 focus:outline-none"
          />
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
                  :src="coverImageUrl(v.view_id, v.cover_image, v.resolved_cover || 'vlib_default_tv')"
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
                      :title="ruleBadges(v.filter_rules).join(' · ') || '无附加过滤条件（匹配全库）'"
                    >
                      排序 #{{ v.sort_order }} · {{ ruleBadges(v.filter_rules).length ? `${ruleBadges(v.filter_rules).length} 项规则` : '匹配全库' }}
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
              @click="openInfoModal(v)" :disabled="saving"
              title="设置名称与封面"
              :aria-label="`设置智能视图 ${v.name} 名称与封面`"
              class="cursor-pointer px-2.5 py-1.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-300 hover:text-emerald-300 transition-colors flex items-center justify-center disabled:opacity-50"
            >
              <Pencil class="w-3.5 h-3.5" aria-hidden="true" />
            </button>
            <button
              type="button"
              @click="openRulesModal(v)" :disabled="saving"
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

    <!-- 智能视图编辑 / 创建弹窗（按入口拆分：基本信息与封面 / 筛选规则 / 新建两步向导） -->
    <div
      v-if="modal.visible"
      role="dialog"
      aria-modal="true"
      aria-labelledby="vlib-modal-title"
      class="fixed inset-0 bg-black/75 backdrop-blur-xs flex items-center justify-center z-50 p-4"
      @click.self="!saving && (modal.visible = false)"
    >
      <div
        class="card p-6 rounded-2xl w-full space-y-4 max-h-[92vh] overflow-y-auto"
        :class="showRulesSection ? 'max-w-3xl' : 'max-w-2xl'"
      >
        <div class="flex items-center justify-between gap-3 border-b border-slate-800 pb-3">
          <div class="flex items-center gap-3 min-w-0">
            <h3 id="vlib-modal-title" class="text-base font-bold text-white flex items-center gap-2 truncate">
              <Sparkles class="w-4 h-4 text-emerald-400 shrink-0" aria-hidden="true" />
              <span v-if="modal.mode === 'info'" class="truncate">视图信息与封面 · {{ modal.name }}</span>
              <span v-else-if="modal.mode === 'rules'" class="truncate">编辑筛选规则 · {{ modal.name }}</span>
              <span v-else>新建智能视图</span>
            </h3>
            <div v-if="modal.mode === 'create'" class="hidden sm:flex items-center gap-1.5 text-[11px] shrink-0">
              <span
                class="px-2 py-0.5 rounded-md border"
                :class="modal.step === 1 ? 'bg-emerald-500/15 border-emerald-500/40 text-emerald-300 font-medium' : 'border-slate-800 text-slate-400'"
              >
                1. 基本信息
              </span>
              <span class="text-slate-600">→</span>
              <span
                class="px-2 py-0.5 rounded-md border"
                :class="modal.step === 2 ? 'bg-emerald-500/15 border-emerald-500/40 text-emerald-300 font-medium' : 'border-slate-800 text-slate-400'"
              >
                2. 筛选规则
              </span>
            </div>
          </div>
          <button
            type="button"
            aria-label="关闭智能视图弹窗"
            @click="modal.visible = false" :disabled="saving"
            class="cursor-pointer text-slate-400 hover:text-white p-1 rounded-lg hover:bg-slate-800 transition-colors shrink-0"
          >
            <X class="w-4 h-4" aria-hidden="true" />
          </button>
        </div>

        <!-- 基本信息与封面区 -->
        <div v-if="showInfoSection" class="grid grid-cols-1 sm:grid-cols-2 gap-4 text-xs">
          <div class="sm:col-span-2">
            <label for="vlib-name" class="block text-slate-300 mb-1.5">视图名称</label>
            <input
              id="vlib-name"
              v-model="modal.name"
              type="text"
              placeholder="例如：TMDB 8.0+ 高分佳作（显示在客户端首页）"
              class="w-full bg-slate-950 border border-slate-700 focus:border-emerald-500 rounded-xl px-3.5 py-2 text-sm"
            />
          </div>

          <div>
            <label for="vlib-icon-type" class="block text-slate-300 mb-1.5">展示样式</label>
            <select
              id="vlib-icon-type"
              v-model="modal.iconType"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-sm"
            >
              <option value="tvshows">剧集样式</option>
              <option value="movies">电影样式</option>
            </select>
          </div>

          <div>
            <label for="vlib-sort-order" class="block text-slate-300 mb-1.5">排序权重（数字越小越靠前）</label>
            <input
              id="vlib-sort-order"
              v-model.number="modal.sortOrder"
              type="number"
              class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-sm font-mono tabular-nums"
            />
          </div>

          <div class="sm:col-span-2 space-y-2">
            <div class="flex items-center justify-between">
              <label class="block text-slate-300">客户端封面</label>
              <span class="text-[11px] text-slate-500">16:9 横版，可选动态拼贴或内置图</span>
            </div>
            <div class="grid grid-cols-2 sm:grid-cols-3 gap-2">
              <button
                type="button"
                @click="modal.coverImage = ''"
                class="cursor-pointer rounded-xl border p-2 text-left transition-all"
                :class="
                  !modal.coverImage
                    ? 'border-emerald-500 bg-emerald-950/40 text-emerald-200'
                    : 'border-slate-800 bg-slate-950/70 text-slate-400 hover:border-slate-700'
                "
              >
                <div class="text-[11px] font-semibold">默认封面</div>
                <div class="text-[10px] text-slate-500 mt-0.5">按名称匹配内置图</div>
              </button>
              <button
                type="button"
                @click="modal.coverImage = 'auto'"
                class="cursor-pointer rounded-xl border p-2 text-left transition-all"
                :class="
                  modal.coverImage === 'auto'
                    ? 'border-amber-500 bg-amber-950/40 text-amber-200'
                    : 'border-slate-800 bg-slate-950/70 text-slate-400 hover:border-slate-700'
                "
              >
                <div class="text-[11px] font-semibold">动态首图</div>
                <div class="text-[10px] text-slate-500 mt-0.5">取最高分匹配剧照</div>
              </button>
              <button
                v-for="s in collageOptions"
                :key="s.key"
                type="button"
                @click="modal.coverImage = s.key"
                class="cursor-pointer rounded-xl border p-2 text-left transition-all"
                :class="
                  modal.coverImage === s.key
                    ? 'border-indigo-400 bg-indigo-950/40 text-indigo-200'
                    : 'border-slate-800 bg-slate-950/70 text-slate-400 hover:border-slate-700'
                "
              >
                <div class="text-[11px] font-semibold">{{ s.label }}</div>
                <div class="text-[10px] text-slate-500 mt-0.5">{{ s.desc }}</div>
              </button>
            </div>
            <div class="grid grid-cols-3 sm:grid-cols-4 gap-2 pt-1">
              <button
                v-for="c in coverOptions"
                :key="c.key"
                type="button"
                @click="modal.coverImage = c.key"
                class="cursor-pointer relative rounded-xl overflow-hidden border aspect-video transition-all group"
                :class="
                  modal.coverImage === c.key
                    ? 'border-emerald-400 ring-2 ring-emerald-500/40'
                    : 'border-slate-800 hover:border-slate-600'
                "
              >
                <img :src="`/covers/${c.key}.jpg`" :alt="c.label" class="w-full h-full object-cover" />
                <div class="absolute inset-x-0 bottom-0 bg-black/75 px-1.5 py-0.5 text-[10px] text-slate-200 truncate">
                  {{ c.label }}
                </div>
              </button>
            </div>
          </div>

          <div class="sm:col-span-2 flex items-center gap-2 pt-1">
            <input
              id="vlib-enabled-cb"
              v-model="modal.isEnabled"
              type="checkbox"
              class="rounded bg-slate-900 border-slate-700 cursor-pointer"
            />
            <label for="vlib-enabled-cb" class="text-xs text-slate-300 cursor-pointer">
              挂载到客户端首页
            </label>
          </div>
        </div>

        <!-- 筛选规则区 -->
        <div v-if="showRulesSection" class="space-y-4">
          <SmartRuleEditor v-model="modal.rules" :options="ruleOptions" />
          <div class="flex items-center justify-between gap-3 text-xs pt-1">
            <button
              type="button"
              @click="previewRules"
              :disabled="previewing || saving"
              class="cursor-pointer px-3.5 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 border border-slate-700/80 text-xs font-medium text-emerald-300 transition-colors disabled:opacity-50"
            >
              {{ previewing ? '正在计算…' : '预览匹配数量' }}
            </button>
            <span
              aria-live="polite"
              class="tabular-nums"
              :class="previewCount === 0 ? 'text-amber-300 font-medium' : 'text-slate-300'"
            >
              {{
                previewCount == null
                  ? '修改条件后可预览命中数量'
                  : previewCount === 0
                    ? '当前规则匹配 0 部影视，建议放宽条件'
                    : `匹配 ${previewCount.toLocaleString()} 部影视`
              }}
            </span>
          </div>
        </div>

        <p v-if="formError" role="alert" class="text-sm text-rose-300">{{ formError }}</p>

        <div class="flex justify-end gap-2.5 pt-3 border-t border-slate-800">
          <template v-if="modal.mode === 'create' && modal.step === 1">
            <button
              type="button"
              @click="modal.visible = false" :disabled="saving"
              class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-4 py-2 rounded-xl text-xs font-medium transition-colors"
            >
              取消
            </button>
            <button
              type="button"
              @click="submitModal" :disabled="saving"
              title="不设附加规则，创建后匹配全库"
              class="cursor-pointer bg-slate-800 hover:bg-slate-700 border border-slate-700 px-4 py-2 rounded-xl text-xs font-medium text-slate-200 transition-colors disabled:opacity-50"
            >
              {{ saving ? '正在创建…' : '直接创建' }}
            </button>
            <button
              type="button"
              @click="nextCreateStep" :disabled="saving"
              class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 px-5 py-2 rounded-xl text-xs font-semibold text-white transition-colors"
            >
              下一步：配置规则
            </button>
          </template>

          <template v-else-if="modal.mode === 'create' && modal.step === 2">
            <button
              type="button"
              @click="formError = ''; modal.step = 1" :disabled="saving"
              class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-4 py-2 rounded-xl text-xs font-medium transition-colors"
            >
              上一步
            </button>
            <button
              type="button"
              @click="submitModal" :disabled="saving"
              class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 px-5 py-2 rounded-xl text-xs font-semibold text-white transition-colors disabled:opacity-50"
            >
              {{ saving ? '正在创建…' : '创建智能视图' }}
            </button>
          </template>

          <template v-else>
            <button
              type="button"
              @click="modal.visible = false" :disabled="saving"
              class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-4 py-2 rounded-xl text-xs font-medium transition-colors"
            >
              取消
            </button>
            <button
              type="button"
              @click="submitModal" :disabled="saving"
              class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 px-5 py-2 rounded-xl text-xs font-semibold text-white transition-colors disabled:opacity-50"
            >
              {{ saving ? '正在保存…' : (modal.mode === 'info' ? '保存设置' : '保存规则') }}
            </button>
          </template>
        </div>
      </div>
    </div>

  </section>
</template>

<script setup lang="ts">
import { ref, reactive, computed, onMounted, watch } from 'vue'
import { Sparkles, Plus, Film, Tv, SlidersHorizontal, Trash2, X, Image as ImageIcon, Pencil, Power } from 'lucide-vue-next'
import { adminApi } from '../api/admin'
import { useToast, useConfirm } from '../composables/useToast'

import SmartRuleEditor from './SmartRuleEditor.vue'
import { validateSmartRule } from '../api/smartViewRules'
import { COUNTRY_NAME_MAP, LANGUAGE_NAME_MAP, TV_TYPE_NAME_MAP, WATCH_TYPE_NAME_MAP } from '../api/tmdbMappings'

const toast = useToast()
const confirmModal = useConfirm()
const loading = ref(false)
const saving = ref(false)
const seeding = ref(false)
const backfilling = ref(false)
const baseSaving = ref(false)
const previewing = ref(false)
const togglingId = ref<number | string | null>(null)
const deletingId = ref<number | string | null>(null)
const listFilter = ref<'all' | 'enabled' | 'disabled'>('all')
const listQuery = ref('')
const previewCount = ref<number | null>(null)
const formError = ref('')
const ruleOptions = ref<any>({ networks: [], companies: [], collections: [], metadata: {} })
const libraries = ref<any[]>([])
const coverBust = ref(Date.now())

const filterTabs = computed(() => {
  const total = libraries.value.length
  const enabled = libraries.value.filter(v => v.is_enabled).length
  return [
    { key: 'all' as const, label: '全部', count: total },
    { key: 'enabled' as const, label: '首页已挂载', count: enabled },
    { key: 'disabled' as const, label: '已停用', count: total - enabled }
  ]
})

const filteredLibraries = computed(() => {
  const q = listQuery.value.trim().toLowerCase()
  return libraries.value.filter(v => {
    if (listFilter.value === 'enabled' && !v.is_enabled) return false
    if (listFilter.value === 'disabled' && v.is_enabled) return false
    if (!q) return true
    const nameMatch = String(v.name || '').toLowerCase().includes(q)
    const badgeMatch = ruleBadges(v.filter_rules).some(b => b.toLowerCase().includes(q))
    return nameMatch || badgeMatch
  })
})

const metadataPercent = computed(() => {
  const total = Number(ruleOptions.value?.metadata?.total || 0)
  const enriched = Number(ruleOptions.value?.metadata?.enriched || 0)
  if (total <= 0) return 0
  return Math.min(100, Math.round((enriched / total) * 100))
})

const baseLibraryCovers = ref([
  { id: 'lib_movie', name: '电影', cover_image: null },
  { id: 'lib_tv', name: '电视剧', cover_image: null },
  { id: 'lib_anime', name: '动漫', cover_image: null },
  { id: 'lib_doc', name: '纪录片', cover_image: null },
  { id: 'lib_variety', name: '综艺', cover_image: null }
])

const baseModal = reactive({
  visible: false,
  id: '',
  name: '',
  coverImage: ''
})

function coverImageUrl(viewId, coverImage, fallbackKey) {
  if (isDynamicCover(coverImage)) {
    return `/Items/${viewId}/Images/Primary?tag=${encodeURIComponent(coverImage)}&_t=${coverBust.value}`
  }
  const key = coverImage || fallbackKey
  return `/covers/${key}.jpg?_t=${coverBust.value}`
}

function openBaseCoverModal(lib) {
  baseModal.id = lib.id
  baseModal.name = lib.name
  baseModal.coverImage = lib.cover_image || ''
  baseModal.visible = true
}

async function submitBaseCoverModal() {
  if (baseSaving.value) return
  baseSaving.value = true
  try {
    const res = await adminApi.saveBaseLibraryCover({
      id: baseModal.id,
      cover_image: baseModal.coverImage || null
    })
    toast.success(res.message)
    baseModal.visible = false
    coverBust.value = Date.now()
    await loadBaseLibraries()
  } catch (err) {
    toast.error('保存失败：' + err.message)
  } finally { baseSaving.value = false }
}

const collageOptions = [
  { key: 'collage_cards', label: '立体叠放卡片', desc: '模糊底图 + 微倾卡片' },
  { key: 'collage_diagonal', label: '斜切电影条带', desc: '4图13°斜切 + 暗角' },
  { key: 'collage_hero', label: '1主3辅非对称', desc: '左主剧照 + 右3缩略图' },
  { key: 'collage_blended', label: '羽化融合4连', desc: '4海报渐变无缝拼接' }
]

function isDynamicCover(val) {
  return val === 'auto' || (typeof val === 'string' && val.startsWith('collage_'))
}

function dynamicCoverLabel(val) {
  if (val === 'auto') return '动态首图'
  const found = collageOptions.find(o => o.key === val)
  return found ? found.label : '动态拼贴'
}

const coverOptions = [
  { key: 'vlib_top_rated', label: '高分佳作' },
  { key: 'vlib_streaming', label: '流媒体剧场' },
  { key: 'vlib_chinese', label: '华语热播' },
  { key: 'vlib_kr_jp', label: '日韩精选' },
  { key: 'vlib_latest', label: '最新院线' },
  { key: 'vlib_default_movies', label: '电影视图' },
  { key: 'vlib_default_tv', label: '剧集视图' },
  { key: 'lib_movie', label: '影院胶片' },
  { key: 'lib_tv', label: '电视光影' },
  { key: 'lib_anime', label: '二次元星空' },
  { key: 'lib_doc', label: '自然山海' },
  { key: 'lib_variety', label: '舞台灯光' }
]

const modal = reactive({
  visible: false,
  mode: 'create' as 'info' | 'rules' | 'create',
  step: 1 as 1 | 2,
  id: null as number | string | null,
  name: '',
  iconType: 'tvshows',
  coverImage: '',
  sortOrder: 10,
  isEnabled: true,
  rules: {} as any
})

const showInfoSection = computed(() =>
  modal.mode === 'info' || (modal.mode === 'create' && modal.step === 1)
)
const showRulesSection = computed(() =>
  modal.mode === 'rules' || (modal.mode === 'create' && modal.step === 2)
)

function formatCountries(list: string[]) {
  return list.map(code => COUNTRY_NAME_MAP[String(code).toUpperCase()] || code).join('、')
}

function formatLanguages(list: string[]) {
  return list.map(code => LANGUAGE_NAME_MAP[String(code).toLowerCase()] || code).join('、')
}

function resolveOptionNames(sourceKey: string, ids: number[]) {
  const list = ruleOptions.value?.[sourceKey] || []
  const map = new Map(list.map((o: any) => [o.id, o.name]))
  return ids.map(id => map.get(id) || `#${id}`).join('、')
}

function ruleBadges(r: any = {}) {
  const badges: string[] = []
  if (r.media_type) badges.push(`类型：${r.media_type === 'movie' ? '电影' : '剧集'}`)
  if (r.categories?.length) badges.push(`分类：${r.categories.join('、')}`)
  if (r.countries?.length) badges.push(`地区：${formatCountries(r.countries)}`)
  if (r.networks?.length) badges.push(`平台：${r.networks.join('、')}`)
  if (r.genres?.length) badges.push(`流派：${r.genres.join('、')}`)
  if (r.genres_all) badges.push('流派全匹配')
  if (r.min_rating != null) badges.push(`评分 ≥ ${r.min_rating}`)
  if (r.max_rating != null) badges.push(`评分 ≤ ${r.max_rating}`)
  if (r.year_min != null || r.year_max != null) {
    badges.push(`年份：${r.year_min || '*'} ~ ${r.year_max || '*'}`)
  }
  if (r.keyword) badges.push(`关键词："${r.keyword}"`)
  if (r.search_overview) badges.push('含剧情简介检索')
  if (r.languages?.length) badges.push(`语言：${formatLanguages(r.languages)}`)
  if (r.spoken_languages?.length) badges.push(`对白：${formatLanguages(r.spoken_languages)}`)
  if (r.min_votes != null) badges.push(`投票 ≥ ${r.min_votes}`)
  if (r.statuses?.length) badges.push(`状态：${r.statuses.map((s: string) => s === 'Ended' ? '已完结' : s === 'Returning Series' ? '持续更新' : s === 'Canceled' ? '已取消' : s === 'In Production' ? '制作中' : s === 'Planned' ? '计划中' : s).join('、')}`)
  if (r.tv_types?.length) badges.push(`节目：${r.tv_types.map((t: string) => TV_TYPE_NAME_MAP[t] || t).join('、')}`)
  if (r.local_complete) badges.push(r.local_complete === 'all' ? '全剧已完结收齐' : '已播集已收齐')
  if (r.version_tags?.length) badges.push(`版本：${r.version_tags.join(' + ')}`)
  if (r.runtime_min != null) badges.push(`时长 ≥ ${r.runtime_min} 分钟`)
  if (r.runtime_max != null) badges.push(`时长 ≤ ${r.runtime_max} 分钟`)
  if (r.episodes_min != null) badges.push(`集数 ≥ ${r.episodes_min} 集`)
  if (r.episodes_max != null) badges.push(`集数 ≤ ${r.episodes_max} 集`)
  if (r.seasons_min != null) badges.push(`季数 ≥ ${r.seasons_min} 季`)
  if (r.seasons_max != null) badges.push(`季数 ≤ ${r.seasons_max} 季`)
  if (r.min_popularity != null) badges.push(`热度 ≥ ${r.min_popularity}`)
  if (r.premiered_within_days != null) badges.push(`近 ${r.premiered_within_days} 天首播`)
  if (r.added_within_days != null) badges.push(`近 ${r.added_within_days} 天入库`)
  if (r.aired_within_days != null) badges.push(`近 ${r.aired_within_days} 天有新集`)
  if (r.airing_within_days != null) badges.push(r.airing_within_days === 0 ? '今日即将播出' : `未来 ${r.airing_within_days} 天播出`)
  if (r.digital_within_days != null) badges.push(`近 ${r.digital_within_days} 天数字发行`)
  for (const [key, source, label] of [
    ['network_ids', 'networks', '播出网络'],
    ['company_ids', 'companies', '制作公司'],
    ['collection_ids', 'collections', '合集'],
    ['keyword_ids', 'keywords', '题材词'],
    ['director_ids', 'directors', '导演'],
    ['person_ids', 'cast', '演职员'],
    ['watch_provider_ids', 'watch_providers', '观看平台']
  ]) {
    if (r[key]?.length) badges.push(`${label}：${resolveOptionNames(source, r[key])}`)
  }
  if (r.watch_region) badges.push(`平台地区：${COUNTRY_NAME_MAP[String(r.watch_region).toUpperCase()] || r.watch_region}`)
  if (r.watch_types?.length) badges.push(`观看方式：${r.watch_types.map((t: string) => WATCH_TYPE_NAME_MAP[t] || t).join('、')}`)
  if (r.cert_region || r.certifications?.length) {
    const regionName = r.cert_region ? (COUNTRY_NAME_MAP[String(r.cert_region).toUpperCase()] || r.cert_region) : ''
    badges.push(`分级：${[regionName, ...(r.certifications || [])].filter(Boolean).join(' ')}`)
  }
  if (r.exclude_countries?.length) badges.push(`排除地区：${formatCountries(r.exclude_countries)}`)
  if (r.exclude_genres?.length) badges.push(`排除流派：${r.exclude_genres.join('、')}`)
  if (r.exclude_networks?.length) badges.push(`排除网络：${r.exclude_networks.join('、')}`)
  if (r.has_collection != null) badges.push(r.has_collection ? '属于电影合集' : '未归入合集')
  if (r.premiered_after || r.premiered_before) badges.push(`首播：${r.premiered_after || '*'} ~ ${r.premiered_before || '*'}`)
  if (r.sort_by) {
    const sortLabels: Record<string, string> = { PremiereDate: '发行年份', CommunityRating: '评分', Popularity: '热度', DateCreated: '入库时间', SortName: '片名' }
    badges.push(`排序：${sortLabels[r.sort_by] || r.sort_by}`)
  }
  if (r.any?.length) badges.push(`任选 ${r.any.length} 组条件`)
  return badges
}

async function loadBaseLibraries() {
  try {
    const data = await adminApi.listBaseLibraryCovers()
    if (Array.isArray(data.libraries) && data.libraries.length > 0) {
      baseLibraryCovers.value = data.libraries
    }
  } catch (err) {
    // 忽略初始加载失败，保留默认基础库列表
  }
}

async function loadLibraries() {
  loading.value = true
  try {
    const data = await adminApi.listVirtualLibraries()
    libraries.value = data.libraries || []
  } catch (err) {
    toast.error('加载智能视图失败：' + err.message)
  } finally {
    loading.value = false
  }
}

function openModal(item: any = null, mode: 'info' | 'rules' | 'create' = 'create', step: 1 | 2 = 1) {
  const r = item ? item.filter_rules || {} : {}
  modal.mode = mode
  modal.step = step
  modal.id = item ? item.id : null
  modal.name = item ? item.name : ''
  modal.iconType = item ? item.icon_type : 'tvshows'
  modal.coverImage = item ? (item.cover_image || '') : ''
  modal.sortOrder = item ? item.sort_order : 10
  modal.isEnabled = item ? item.is_enabled : true
  modal.rules = JSON.parse(JSON.stringify(r))
  formError.value = ''
  previewCount.value = null
  modal.visible = true
}

function openInfoModal(item: any) {
  openModal(item, 'info', 1)
}

function openRulesModal(item: any) {
  openModal(item, 'rules', 2)
}

function openCreateModal() {
  openModal(null, 'create', 1)
}

function nextCreateStep() {
  formError.value = ''
  try {
    if (!modal.name.trim() || Array.from(modal.name.trim()).length > 64) throw new Error('视图名称须为 1～64 个字符')
    if (!Number.isInteger(Number(modal.sortOrder))) throw new Error('排序权重须为整数')
  } catch (err: any) {
    formError.value = err.message
    return
  }
  modal.step = 2
}

watch(() => modal.rules, () => { previewCount.value = null }, { deep: true })

async function previewRules() {
  if (previewing.value) return
  previewing.value = true
  formError.value = ''
  const snapshot = JSON.stringify(modal.rules)
  try {
    validateSmartRule(modal.rules)
    const result = await adminApi.previewSmartView(modal.rules)
    if (snapshot === JSON.stringify(modal.rules)) previewCount.value = result.matched_count
  } catch (err) { formError.value = err.message }
  finally { previewing.value = false }
}

async function submitModal() {
  if (saving.value) return
  formError.value = ''
  try {
    if (!modal.name.trim() || Array.from(modal.name.trim()).length > 64) throw new Error('视图名称须为 1～64 个字符')
    if (!Number.isInteger(Number(modal.sortOrder))) throw new Error('排序权重须为整数')
    validateSmartRule(modal.rules)
  } catch (err) { formError.value = err.message; return }
  saving.value = true
  try {
    const res = await adminApi.saveVirtualLibrary({
      id: modal.id, name: modal.name.trim(), icon_type: modal.iconType,
      cover_image: modal.coverImage || null, sort_order: Number(modal.sortOrder) || 0,
      is_enabled: modal.isEnabled, filter_rules: JSON.parse(JSON.stringify(modal.rules))
    })
    toast.success(res.message)
    modal.visible = false
    coverBust.value = Date.now()
    await loadLibraries()
  } catch (err) { formError.value = '保存失败：' + err.message }
  finally { saving.value = false }
}

async function loadRuleOptions() {
  try { ruleOptions.value = await adminApi.smartViewOptions() }
  catch (err) { toast.error('加载筛选选项失败：' + err.message) }
}
async function backfillMetadata() {
  if (backfilling.value) return
  const ok = await confirmModal({ title: '补齐 TMDB 扩展信息', message: '要为本地已刮削且未锁定的影视补齐语言、投票数、时长等扩展信息吗？任务会分批请求 TMDB，可在关闭全库自动刮削时执行。', confirmText: '开始补齐' })
  if (!ok) return
  backfilling.value = true
  try { const res = await adminApi.backfillSmartViewMetadata(); toast.success(res.message); await loadRuleOptions() }
  catch (err) { toast.error('补齐失败：' + err.message) }
  finally { backfilling.value = false }
}

async function handleSeedPresets() {
  if (seeding.value) return
  seeding.value = true
  try {
    const data = await adminApi.seedPresetVirtualLibraries()
    toast.success(data.message)
    await loadLibraries()
  } catch (err) {
    toast.error('生成预设失败：' + err.message)
  } finally { seeding.value = false }
}

async function toggleEnabled(v: any) {
  if (togglingId.value != null) return
  togglingId.value = v.id
  const nextEnabled = !v.is_enabled
  try {
    await adminApi.saveVirtualLibrary({
      id: v.id,
      name: v.name,
      icon_type: v.icon_type,
      cover_image: v.cover_image || null,
      sort_order: Number(v.sort_order) || 0,
      is_enabled: nextEnabled,
      filter_rules: JSON.parse(JSON.stringify(v.filter_rules || {}))
    })
    v.is_enabled = nextEnabled
    toast.success(nextEnabled ? `已挂载「${v.name}」至首页` : `已停用「${v.name}」`)
  } catch (err: any) {
    toast.error('切换状态失败：' + err.message)
  } finally {
    togglingId.value = null
  }
}

async function handleDelete(v: any) {
  if (deletingId.value != null) return
  const ok = await confirmModal({
    title: '删除智能视图',
    message: `确认删除智能视图「${v.name}」吗？该操作不会影响在库影视资源。`,
    confirmText: '确认删除',
    danger: true
  })
  if (!ok) return
  deletingId.value = v.id
  try {
    await adminApi.deleteVirtualLibrary(v.id)
    toast.info(`已删除智能视图「${v.name}」`)
    await loadLibraries()
  } catch (err: any) {
    toast.error('删除失败：' + err.message)
  } finally {
    deletingId.value = null
  }
}

onMounted(() => {
  loadLibraries()
  loadRuleOptions()
})
</script>
