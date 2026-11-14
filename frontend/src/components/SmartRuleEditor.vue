<script setup lang="ts">
import { ref, computed, watch, nextTick } from 'vue'
import { ChevronDown, ChevronUp } from 'lucide-vue-next'
import SmartRuleRow from './smart-rules/SmartRuleRow.vue'
import {
  SMART_RULE_SECTIONS,
  ALL_SMART_RULE_FIELDS,
  SMART_RULE_FIELD_MAP,
  QUICK_SMART_RULE_FIELDS,
  hasSmartRuleValue
} from '../utils/smartRuleFields.js'

const rule = defineModel<any>({ required: true })
const props = defineProps<{ options: any; depth?: number }>()
const depth = props.depth || 0

const pickerOpen = ref(false)
const pickerSearch = ref('')
const addedKeys = ref<string[]>([])

watch(() => rule.value, (newVal, oldVal) => {
  const currentRule = newVal || {}
  const next: string[] = []
  if (newVal === oldVal) {
    for (const k of addedKeys.value) {
      if (SMART_RULE_FIELD_MAP.has(k)) next.push(k)
    }
  }
  for (const field of ALL_SMART_RULE_FIELDS) {
    if (hasSmartRuleValue(field, currentRule[field.key]) && !next.includes(field.key)) {
      next.push(field.key)
    }
  }
  addedKeys.value = next
}, { immediate: true })

const visibleFields = computed(() =>
  addedKeys.value.map(k => SMART_RULE_FIELD_MAP.get(k)).filter(Boolean)
)

const activeCount = computed(() => {
  const currentRule = rule.value || {}
  return ALL_SMART_RULE_FIELDS.filter(f => hasSmartRuleValue(f, currentRule[f.key])).length
})

function isSourceEmpty(field: any) {
  if (!field.source) return false
  const hasDynamic = (props.options?.[field.source] || []).length > 0
  const hasSaved = (rule.value?.[field.key] || []).length > 0
  return !hasDynamic && !hasSaved
}

const hasAnyEmptySource = computed(() =>
  ALL_SMART_RULE_FIELDS.some(f => isSourceEmpty(f))
)

const filteredSections = computed(() => {
  const q = pickerSearch.value.trim().toLowerCase()
  if (!q) return SMART_RULE_SECTIONS
  return SMART_RULE_SECTIONS
    .map(s => ({
      ...s,
      fields: s.fields.filter((f: any) =>
        f.label.toLowerCase().includes(q) ||
        (f.placeholder || '').toLowerCase().includes(q) ||
        (f.help || '').toLowerCase().includes(q)
      )
    }))
    .filter(s => s.fields.length > 0)
})

function isFieldAdded(key: string) {
  return addedKeys.value.includes(key)
}

async function addField(key: string) {
  if (!addedKeys.value.includes(key)) {
    addedKeys.value.push(key)
  }
  await nextTick()
  const el = document.getElementById(`rule-${depth}-${key}`)
  el?.focus()
}

function removeField(key: string) {
  addedKeys.value = addedKeys.value.filter(k => k !== key)
  if (rule.value && key in rule.value) {
    delete rule.value[key]
  }
}

function clearAllFields() {
  for (const field of ALL_SMART_RULE_FIELDS) {
    if (rule.value && field.key in rule.value) {
      delete rule.value[field.key]
    }
  }
  addedKeys.value = []
}

function addGroup() {
  rule.value.any ||= []
  rule.value.any.push({})
}
</script>

<template>
  <div class="space-y-3 text-sm">
    <!-- 一体化条件面板 -->
    <div class="rounded-xl border border-slate-700/90 bg-slate-950/50 overflow-hidden">
      <!-- 面板顶栏 -->
      <div class="flex items-center justify-between gap-2 px-3.5 py-2.5 bg-slate-900/70 border-b border-slate-800/80">
        <div class="flex items-center gap-2 min-w-0">
          <span class="text-xs font-semibold text-slate-200">
            {{ depth === 0 ? '同时满足以下条件' : '同时满足本组条件' }}
          </span>
          <span class="px-1.5 py-0.5 rounded bg-slate-800 border border-slate-700/80 text-[10px] font-mono text-emerald-300">
            AND
          </span>
          <span v-if="activeCount > 0" class="text-[11px] text-slate-400 tabular-nums">
            · 已设 {{ activeCount }} 项
          </span>
        </div>
        <button
          v-if="visibleFields.length > 0"
          type="button"
          @click="clearAllFields"
          class="cursor-pointer text-[11px] text-slate-400 hover:text-rose-300 transition-colors"
        >
          清空全部
        </button>
      </div>

      <!-- 单行条件列表 -->
      <div v-if="visibleFields.length" class="divide-y divide-slate-800/70">
        <SmartRuleRow
          v-for="field in visibleFields"
          :key="field.key"
          :field="field"
          :rule="rule"
          :options="options"
          :depth="depth"
          @remove="removeField"
        />
      </div>

      <!-- 空状态 -->
      <div v-else class="px-4 py-5 text-center space-y-1">
        <p class="text-xs font-medium text-slate-300">尚未添加筛选条件（当前将匹配全库影视）</p>
        <p class="text-[11px] text-slate-500">点击下方常用条件或「更多条件」按需组合规则</p>
      </div>

      <!-- 面板底栏：常用快捷添加 + 更多条件目录 -->
      <div class="px-3.5 py-2.5 bg-slate-900/40 border-t border-slate-800/80 space-y-2.5">
        <div class="flex flex-wrap items-center gap-1.5">
          <span class="text-[11px] text-slate-400 mr-0.5">常用：</span>
          <button
            v-for="qf in QUICK_SMART_RULE_FIELDS"
            :key="qf.key"
            type="button"
            :disabled="isFieldAdded(qf.key)"
            @click="addField(qf.key)"
            class="cursor-pointer px-2 py-1 rounded-lg border text-[11px] transition-colors"
            :class="
              isFieldAdded(qf.key)
                ? 'border-emerald-500/30 bg-emerald-500/10 text-emerald-300/70 cursor-default'
                : 'border-slate-700/80 bg-slate-900 hover:border-emerald-500/50 hover:text-emerald-200 text-slate-300'
            "
          >
            {{ isFieldAdded(qf.key) ? `✓ ${qf.shortLabel || qf.label}` : `+ ${qf.shortLabel || qf.label}` }}
          </button>

          <button
            type="button"
            @click="pickerOpen = !pickerOpen"
            class="cursor-pointer ml-auto px-2.5 py-1 rounded-lg bg-slate-800 hover:bg-slate-700 border border-slate-700 text-[11px] font-medium text-emerald-300 transition-colors inline-flex items-center gap-1"
          >
            <span>{{ pickerOpen ? '收起条件目录' : `更多条件（${ALL_SMART_RULE_FIELDS.length} 项）` }}</span>
            <ChevronUp v-if="pickerOpen" class="w-3.5 h-3.5" aria-hidden="true" />
            <ChevronDown v-else class="w-3.5 h-3.5" aria-hidden="true" />
          </button>
        </div>

        <!-- 可搜索条件目录 -->
        <div v-if="pickerOpen" class="pt-2.5 border-t border-slate-800/80 space-y-2.5">
          <input
            v-model="pickerSearch"
            type="search"
            aria-label="搜索筛选条件"
            placeholder="搜索条件名称，例如：评分、地区、语言、4K、完结、平台…"
            class="w-full px-3 py-1.5 rounded-lg bg-slate-900 border border-slate-700 text-xs text-slate-200 placeholder:text-slate-500 focus:outline-none focus:border-emerald-500"
          />
          <p v-if="hasAnyEmptySource" class="text-[11px] text-amber-300/80 px-0.5">
            提示：标有「暂无本地数据」的条件需先在列表页点击「从本地刮削补齐」后才会有可选项。
          </p>

          <div class="max-h-60 overflow-y-auto space-y-3 pr-1">
            <div v-for="group in filteredSections" :key="group.title" class="space-y-1.5">
              <div class="text-[11px] font-semibold text-slate-400 px-0.5">{{ group.title }}</div>
              <div class="grid grid-cols-1 sm:grid-cols-2 gap-1.5">
                <button
                  v-for="field in group.fields"
                  :key="field.key"
                  type="button"
                  @click="addField(field.key)"
                  class="cursor-pointer flex items-center justify-between gap-2 px-2.5 py-1.5 rounded-lg border text-left text-xs transition-colors"
                  :class="
                    isFieldAdded(field.key)
                      ? 'border-emerald-500/35 bg-emerald-950/25 text-emerald-200'
                      : isSourceEmpty(field)
                        ? 'border-slate-800 bg-slate-950/50 text-slate-400 hover:border-slate-700'
                        : 'border-slate-800 bg-slate-900/70 text-slate-200 hover:border-emerald-500/40'
                  "
                >
                  <span class="truncate">{{ field.label }}</span>
                  <span
                    v-if="isFieldAdded(field.key)"
                    class="text-[10px] px-1.5 py-0.5 rounded bg-emerald-500/20 text-emerald-300 shrink-0"
                  >
                    已添加
                  </span>
                  <span
                    v-else-if="isSourceEmpty(field)"
                    class="text-[10px] px-1.5 py-0.5 rounded bg-amber-500/10 text-amber-300/80 shrink-0"
                  >
                    暂无本地数据
                  </span>
                </button>
              </div>
            </div>
            <p v-if="filteredSections.length === 0" class="text-xs text-slate-500 text-center py-4">
              没有匹配「{{ pickerSearch }}」的筛选条件
            </p>
          </div>
        </div>
      </div>
    </div>

    <!-- 任选条件组（OR） -->
    <details v-if="depth < 2" class="rounded-xl border border-slate-700/90 bg-slate-950/40" :open="!!rule.any?.length">
      <summary class="cursor-pointer px-3.5 py-2.5 text-xs font-medium text-slate-200 flex items-center justify-between gap-2 select-none">
        <span class="flex items-center gap-2">
          <span>任选条件组</span>
          <span class="px-1.5 py-0.5 rounded bg-slate-800 border border-slate-700/80 text-[10px] font-mono text-amber-300">OR</span>
          <span class="text-slate-400 font-normal">{{ rule.any?.length ? `· ${rule.any.length} 组` : '· 可选' }}</span>
        </span>
      </summary>
      <div class="p-3.5 pt-1 space-y-3">
        <p class="text-slate-400 text-xs leading-relaxed">满足上方条件后，再满足下面任意一组（OR）。例如：美国电影，或者日本动画。</p>
        <div v-for="(group, index) in rule.any || []" :key="index" class="border-l-2 border-emerald-500/60 pl-3 space-y-2">
          <div class="flex items-center justify-between">
            <span class="text-xs font-medium text-slate-200">条件组 {{ index + 1 }}</span>
            <button
              type="button"
              @click="rule.any.splice(index, 1)"
              class="cursor-pointer text-xs text-rose-300 hover:bg-rose-500/15 px-2.5 py-1 rounded-lg transition-colors"
            >
              删除这组
            </button>
          </div>
          <SmartRuleEditor v-model="rule.any[index]" :options="options" :depth="depth + 1" />
        </div>
        <button
          type="button"
          :disabled="(rule.any?.length || 0) >= 8"
          @click="addGroup"
          class="cursor-pointer px-3.5 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 text-xs font-medium text-emerald-300 transition-colors disabled:opacity-50"
        >
          添加任选条件组
        </button>
      </div>
    </details>
  </div>
</template>
