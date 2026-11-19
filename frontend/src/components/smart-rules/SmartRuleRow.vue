<script setup lang="ts">
import { ref, computed } from 'vue'
import { X } from 'lucide-vue-next'

const props = defineProps<{
  field: any
  rule: any
  options: any
  depth: number
  backfilling?: boolean
}>()

const emit = defineEmits<{
  remove: [key: string]
  backfill: []
  openTasks: []
}>()

const filterQuery = ref('')
const customInput = ref('')

const inputId = computed(() => `rule-${props.depth}-${props.field.key}`)

function presetDisplayLabel(val: string) {
  const found = (props.field.presetOptions || []).find(
    (o: any) => o.value.toLowerCase() === String(val).toLowerCase()
  )
  return found ? found.label : val
}

const availablePresetOptions = computed(() => {
  const selected = new Set(
    (props.rule[props.field.key] || []).map((v: string) => String(v).toLowerCase())
  )
  return (props.field.presetOptions || []).filter(
    (o: any) => !selected.has(o.value.toLowerCase())
  )
})

function addPresetValue(val: string) {
  if (!val) return
  const key = props.field.key
  const current = Array.isArray(props.rule[key]) ? [...props.rule[key]] : []
  if (!current.some(v => String(v).toLowerCase() === val.toLowerCase())) {
    current.push(val)
    props.rule[key] = current
  }
}

function removePresetValue(val: string) {
  const key = props.field.key
  const current = Array.isArray(props.rule[key]) ? [...props.rule[key]] : []
  props.rule[key] = current.filter(v => v !== val)
}

function commitCustomValue() {
  const raw = customInput.value.trim()
  if (!raw) return
  for (const part of raw.split(/[,，]/).map(s => s.trim()).filter(Boolean)) {
    addPresetValue(part)
  }
  customInput.value = ''
}

function read() {
  const value = props.rule[props.field.key]
  if (props.field.singleList) return value?.[0] || ''
  if (props.field.boolean) return value == null ? '' : String(value)
  return value ?? ''
}

function write(value: string) {
  const f = props.field
  props.rule[f.key] = f.singleList
    ? (value ? [value] : [])
    : f.boolean
      ? (value === '' ? null : value === 'true')
      : f.type === 'number'
        ? (value === '' ? null : Number(value))
        : (value || null)
}

function writeCheckbox(checked: boolean) {
  if (checked) props.rule[props.field.key] = true
  else delete props.rule[props.field.key]
}

const mergedOptions = computed(() => {
  const source = props.field.source
  if (!source) return []
  const values = [...(props.options?.[source] || [])]
  for (const id of props.rule[props.field.key] || []) {
    if (!values.some(v => v.id === id)) values.push({ id, name: `TMDB #${id}` })
  }
  return values
})

const filteredOptions = computed(() => {
  const query = filterQuery.value.trim().toLowerCase()
  const selected = new Set(props.rule[props.field.key] || [])
  const list = mergedOptions.value
  const filtered = query ? list.filter(o => String(o.name).toLowerCase().includes(query)) : list
  return [...filtered].sort((a, b) => Number(selected.has(b.id)) - Number(selected.has(a.id)))
})

function isIdSelected(id: number) {
  return (props.rule[props.field.key] || []).includes(id)
}

function toggleId(id: number) {
  const key = props.field.key
  const current = Array.isArray(props.rule[key]) ? [...props.rule[key]] : []
  const idx = current.indexOf(id)
  if (idx >= 0) current.splice(idx, 1)
  else current.push(id)
  props.rule[key] = current
}
</script>

<template>
  <div class="flex flex-col sm:flex-row sm:items-start gap-2 sm:gap-3 px-3.5 py-2.5">
    <!-- 左列：条件名称 -->
    <div class="sm:w-40 shrink-0 sm:pt-1.5 flex items-center justify-between sm:justify-start gap-1.5">
      <label :for="inputId" class="text-xs font-medium text-slate-300">
        {{ field.label }}
      </label>
      <span
        v-if="field.source && (rule[field.key]?.length || 0) > 0"
        class="px-1.5 py-0.5 rounded bg-emerald-500/15 text-[10px] text-emerald-300 tabular-nums"
      >
        {{ rule[field.key].length }}
      </span>
    </div>

    <!-- 中列：控件与说明 -->
    <div class="flex-1 min-w-0 space-y-1.5">
      <!-- 1. 复选框 -->
      <label v-if="field.type === 'checkbox'" class="flex items-center gap-2 text-xs text-slate-200 cursor-pointer py-1">
        <input
          :id="inputId"
          type="checkbox"
          :checked="!!rule[field.key]"
          @change="writeCheckbox(($event.target as HTMLInputElement).checked)"
          class="rounded bg-slate-900 border-slate-700 cursor-pointer"
        />
        <span>{{ field.checkboxLabel || field.label }}</span>
      </label>

      <!-- 2. 单选下拉框 -->
      <select
        v-else-if="field.options"
        :id="inputId"
        :value="read()"
        @change="write(($event.target as HTMLSelectElement).value)"
        class="rule-input"
      >
        <option value="">不限</option>
        <option v-for="option in field.options" :key="option.value" :value="option.value">
          {{ option.label }}
        </option>
      </select>

      <!-- 3. 预设多选下拉框 -->
      <div v-else-if="field.presetOptions" class="space-y-1.5">
        <div v-if="(rule[field.key]?.length || 0) > 0" class="flex flex-wrap gap-1.5">
          <span
            v-for="val in rule[field.key]"
            :key="val"
            class="inline-flex items-center gap-1 px-2 py-0.5 rounded-lg bg-emerald-950/50 border border-emerald-500/35 text-xs text-emerald-200"
          >
            <span>{{ presetDisplayLabel(val) }}</span>
            <button
              type="button"
              :aria-label="`移除 ${presetDisplayLabel(val)}`"
              @click="removePresetValue(val)"
              class="cursor-pointer text-emerald-300/70 hover:text-rose-300 leading-none"
            >
              <X class="w-3 h-3" aria-hidden="true" />
            </button>
          </span>
        </div>
        <div class="flex gap-1.5">
          <select
            :id="inputId"
            value=""
            @change="addPresetValue(($event.target as HTMLSelectElement).value); ($event.target as HTMLSelectElement).value = ''"
            class="rule-input flex-1"
          >
            <option value="">+ 下拉选择{{ field.shortLabel || field.label }}…</option>
            <option
              v-for="option in availablePresetOptions"
              :key="option.value"
              :value="option.value"
            >
              {{ option.label }}
            </option>
          </select>
          <input
            v-if="field.allowCustom"
            v-model="customInput"
            type="text"
            :placeholder="field.customPlaceholder || '自定义值，回车添加'"
            @keydown.enter.prevent="commitCustomValue"
            class="w-36 sm:w-44 px-2.5 py-1.5 rounded-lg bg-slate-950 border border-slate-700 text-xs text-slate-200 placeholder:text-slate-500 focus:outline-none focus:border-emerald-500"
          />
        </div>
      </div>

      <!-- 4. 动态 TMDB 数据源多选 -->
      <div v-else-if="field.source">
        <div
          v-if="mergedOptions.length === 0"
          class="rounded-lg border border-amber-500/25 bg-amber-950/20 px-3 py-2 text-xs text-amber-200/90 leading-relaxed flex flex-wrap items-center justify-between gap-2"
        >
          <span>当前暂无本地可选数据，需先补齐 TMDB 扩展信息后才可在此勾选。</span>
          <div class="flex items-center gap-2 shrink-0">
            <button
              type="button"
              :disabled="backfilling"
              @click="emit('backfill')"
              class="cursor-pointer px-2.5 py-1 rounded-md bg-emerald-600 hover:bg-emerald-500 text-white text-[11px] font-medium transition-colors disabled:opacity-50"
            >
              {{ backfilling ? '正在加入队列…' : '立即补齐' }}
            </button>
            <button
              type="button"
              @click="emit('openTasks')"
              class="cursor-pointer px-2.5 py-1 rounded-md bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 text-[11px] transition-colors"
            >
              任务中心
            </button>
          </div>
        </div>
        <div v-else class="rounded-xl border border-slate-700/90 bg-slate-950 p-2 space-y-2">
          <input
            v-if="mergedOptions.length > 6"
            v-model="filterQuery"
            type="search"
            :aria-label="`筛选${field.label}`"
            placeholder="输入名称筛选…"
            class="w-full px-2.5 py-1.5 rounded-lg bg-slate-900 border border-slate-700/80 text-xs text-slate-200 placeholder:text-slate-500 focus:outline-none focus:border-emerald-500"
          />
          <div v-if="filteredOptions.length" class="max-h-36 overflow-y-auto space-y-1 pr-1">
            <label
              v-for="option in filteredOptions"
              :key="option.id"
              class="flex items-center gap-2 px-2 py-1 rounded-lg hover:bg-slate-900/90 cursor-pointer text-xs text-slate-200"
            >
              <input
                type="checkbox"
                :checked="isIdSelected(option.id)"
                @change="toggleId(option.id)"
                class="rounded bg-slate-900 border-slate-700 cursor-pointer"
              />
              <span class="truncate">{{ option.name }}</span>
            </label>
          </div>
          <p v-else class="text-xs text-slate-500 px-1 py-2 text-center">未找到匹配「{{ filterQuery }}」的选项</p>
        </div>
      </div>

      <!-- 5. 数字 / 日期 / 文本输入 -->
      <input
        v-else
        :id="inputId"
        :type="field.type || 'text'"
        :value="read()"
        @input="write(($event.target as HTMLInputElement).value)"
        :step="field.step || (field.type === 'number' ? 1 : undefined)"
        :min="field.min"
        :max="field.max"
        :placeholder="field.placeholder || '请输入条件值'"
        class="rule-input"
      />

      <span v-if="field.help" class="block text-[11px] leading-relaxed text-slate-400">
        {{ field.help }}
      </span>
    </div>

    <!-- 右列：移除图标按钮 -->
    <button
      type="button"
      :aria-label="`移除 ${field.label}`"
      title="移除该条件"
      @click="emit('remove', field.key)"
      class="cursor-pointer self-end sm:self-start sm:mt-1.5 p-1.5 rounded-lg text-slate-400 hover:text-rose-300 hover:bg-rose-500/10 transition-colors shrink-0"
    >
      <X class="w-3.5 h-3.5" aria-hidden="true" />
    </button>
  </div>
</template>

<style scoped>
.rule-input { width: 100%; padding: .5rem .7rem; border: 1px solid #334155; border-radius: .65rem; background: #020617; color: #e2e8f0; min-height: 36px; font-size: 0.75rem; }
.rule-input:focus { outline: 2px solid #10b981; outline-offset: 2px; }
</style>
