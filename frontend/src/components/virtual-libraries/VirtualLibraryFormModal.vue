<script setup lang="ts">
import { computed } from 'vue'
import { Sparkles, X } from 'lucide-vue-next'
import SmartRuleEditor from '../SmartRuleEditor.vue'
import { collageOptions, coverOptions } from '../../utils/smartViewLabels.js'

export interface VirtualLibraryModalState {
  visible: boolean
  mode: 'info' | 'rules' | 'create'
  step: 1 | 2
  id: number | string | null
  name: string
  iconType: string
  coverImage: string
  sortOrder: number
  isEnabled: boolean
  rules: any
}

const props = defineProps<{
  modal: VirtualLibraryModalState
  ruleOptions: any
  needsMetadataBackfill?: boolean
  backfilling?: boolean
  saving: boolean
  previewing: boolean
  previewCount: number | null
  formError: string
}>()

const emit = defineEmits<{
  close: []
  submit: []
  nextStep: []
  prevStep: []
  preview: []
  backfill: []
  openTasks: []
}>()

const showInfoSection = computed(() =>
  props.modal.mode === 'info' || (props.modal.mode === 'create' && props.modal.step === 1)
)
const showRulesSection = computed(() =>
  props.modal.mode === 'rules' || (props.modal.mode === 'create' && props.modal.step === 2)
)
</script>

<template>
  <div
    v-if="modal.visible"
    role="dialog"
    aria-modal="true"
    aria-labelledby="vlib-modal-title"
    class="fixed inset-0 bg-black/75 backdrop-blur-xs flex items-center justify-center z-50 p-4"
    @click.self="!saving && emit('close')"
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
          @click="emit('close')"
          :disabled="saving"
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
        <div
          v-if="needsMetadataBackfill"
          class="rounded-xl border border-amber-500/30 bg-amber-950/20 px-3.5 py-2.5 flex flex-wrap items-center justify-between gap-2.5 text-xs"
        >
          <div class="text-slate-300 leading-relaxed">
            <span>
              部分已刮削影视尚未补齐语言、时长、投票数或出品平台等扩展元数据（已补齐
              <strong class="text-amber-300 font-mono tabular-nums">{{ (ruleOptions.metadata?.enriched || 0).toLocaleString() }}</strong>
              /
              <strong class="text-slate-200 font-mono tabular-nums">{{ (ruleOptions.metadata?.total || 0).toLocaleString() }}</strong>
              部）。
            </span>
          </div>
          <div class="flex items-center gap-2 shrink-0">
            <button
              type="button"
              :disabled="backfilling"
              @click="emit('backfill')"
              class="cursor-pointer px-3 py-1.5 rounded-lg bg-emerald-600 hover:bg-emerald-500 text-white font-semibold transition-colors disabled:opacity-50"
            >
              {{ backfilling ? '正在加入队列…' : '立即补齐扩展信息' }}
            </button>
            <button
              type="button"
              @click="emit('openTasks')"
              class="cursor-pointer px-2.5 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 transition-colors"
            >
              前往任务中心
            </button>
          </div>
        </div>
        <SmartRuleEditor
          v-model="modal.rules"
          :options="ruleOptions"
          :backfilling="backfilling"
          @backfill="emit('backfill')"
          @open-tasks="emit('openTasks')"
        />
        <div class="flex items-center justify-between gap-3 text-xs pt-1">
          <button
            type="button"
            @click="emit('preview')"
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
            @click="emit('close')"
            :disabled="saving"
            class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-4 py-2 rounded-xl text-xs font-medium transition-colors"
          >
            取消
          </button>
          <button
            type="button"
            @click="emit('submit')"
            :disabled="saving"
            title="不设附加规则，创建后匹配全库"
            class="cursor-pointer bg-slate-800 hover:bg-slate-700 border border-slate-700 px-4 py-2 rounded-xl text-xs font-medium text-slate-200 transition-colors disabled:opacity-50"
          >
            {{ saving ? '正在创建…' : '直接创建' }}
          </button>
          <button
            type="button"
            @click="emit('nextStep')"
            :disabled="saving"
            class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 px-5 py-2 rounded-xl text-xs font-semibold text-white transition-colors"
          >
            下一步：配置规则
          </button>
        </template>

        <template v-else-if="modal.mode === 'create' && modal.step === 2">
          <button
            type="button"
            @click="emit('prevStep')"
            :disabled="saving"
            class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-4 py-2 rounded-xl text-xs font-medium transition-colors"
          >
            上一步
          </button>
          <button
            type="button"
            @click="emit('submit')"
            :disabled="saving"
            class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 px-5 py-2 rounded-xl text-xs font-semibold text-white transition-colors disabled:opacity-50"
          >
            {{ saving ? '正在创建…' : '创建智能视图' }}
          </button>
        </template>

        <template v-else>
          <button
            type="button"
            @click="emit('close')"
            :disabled="saving"
            class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-4 py-2 rounded-xl text-xs font-medium transition-colors"
          >
            取消
          </button>
          <button
            type="button"
            @click="emit('submit')"
            :disabled="saving"
            class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 px-5 py-2 rounded-xl text-xs font-semibold text-white transition-colors disabled:opacity-50"
          >
            {{ saving ? '正在保存…' : (modal.mode === 'info' ? '保存设置' : '保存规则') }}
          </button>
        </template>
      </div>
    </div>
  </div>
</template>
