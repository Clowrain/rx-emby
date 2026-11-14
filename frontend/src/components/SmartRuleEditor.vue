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
        <div
          v-for="field in visibleFields"
          :key="field.key"
          class="flex flex-col sm:flex-row sm:items-start gap-2 sm:gap-3 px-3.5 py-2.5"
        >
          <!-- 左列：条件名称 -->
          <div class="sm:w-40 shrink-0 sm:pt-1.5 flex items-center justify-between sm:justify-start gap-1.5">
            <label :for="`rule-${depth}-${field.key}`" class="text-xs font-medium text-slate-300">
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
                :id="`rule-${depth}-${field.key}`"
                type="checkbox"
                :checked="!!rule[field.key]"
                @change="writeCheckbox(field.key, ($event.target as HTMLInputElement).checked)"
                class="rounded bg-slate-900 border-slate-700 cursor-pointer"
              />
              <span>{{ field.checkboxLabel || field.label }}</span>
            </label>

            <!-- 2. 单选下拉框（含 TMDB 中文映射） -->
            <select
              v-else-if="field.options"
              :id="`rule-${depth}-${field.key}`"
              :value="read(field)"
              @change="write(field, ($event.target as HTMLSelectElement).value)"
              class="rule-input"
            >
              <option value="">不限</option>
              <option v-for="option in field.options" :key="option.value" :value="option.value">
                {{ option.label }}
              </option>
            </select>

            <!-- 3. 预设多选下拉框（中文展示，存储 TMDB 原文，支持自定义输入） -->
            <div v-else-if="field.presetOptions" class="space-y-1.5">
              <div v-if="(rule[field.key]?.length || 0) > 0" class="flex flex-wrap gap-1.5">
                <span
                  v-for="val in rule[field.key]"
                  :key="val"
                  class="inline-flex items-center gap-1 px-2 py-0.5 rounded-lg bg-emerald-950/50 border border-emerald-500/35 text-xs text-emerald-200"
                >
                  <span>{{ presetDisplayLabel(field, val) }}</span>
                  <button
                    type="button"
                    :aria-label="`移除 ${presetDisplayLabel(field, val)}`"
                    @click="removePresetValue(field.key, val)"
                    class="cursor-pointer text-emerald-300/70 hover:text-rose-300 leading-none"
                  >
                    <X class="w-3 h-3" aria-hidden="true" />
                  </button>
                </span>
              </div>
              <div class="flex gap-1.5">
                <select
                  :id="`rule-${depth}-${field.key}`"
                  value=""
                  @change="addPresetValue(field.key, ($event.target as HTMLSelectElement).value); ($event.target as HTMLSelectElement).value = ''"
                  class="rule-input flex-1"
                >
                  <option value="">+ 下拉选择{{ field.shortLabel || field.label }}…</option>
                  <option
                    v-for="option in availablePresetOptions(field)"
                    :key="option.value"
                    :value="option.value"
                  >
                    {{ option.label }}
                  </option>
                </select>
                <input
                  v-if="field.allowCustom"
                  v-model="customInputs[field.key]"
                  type="text"
                  :placeholder="field.customPlaceholder || '自定义值，回车添加'"
                  @keydown.enter.prevent="commitCustomValue(field.key)"
                  class="w-36 sm:w-44 px-2.5 py-1.5 rounded-lg bg-slate-950 border border-slate-700 text-xs text-slate-200 placeholder:text-slate-500 focus:outline-none focus:border-emerald-500"
                />
              </div>
            </div>

            <!-- 4. 动态 TMDB 数据源多选（含空状态引导） -->
            <div v-else-if="field.source">
              <div
                v-if="mergedOptions(field).length === 0"
                class="rounded-lg border border-amber-500/25 bg-amber-950/20 px-3 py-2 text-xs text-amber-200/90 leading-relaxed"
              >
                当前暂无本地可选数据。请先在智能视图列表页点击「从本地刮削补齐」，补齐扩展信息后即可在此勾选。
              </div>
              <div v-else class="rounded-xl border border-slate-700/90 bg-slate-950 p-2 space-y-2">
                <input
                  v-if="mergedOptions(field).length > 6"
                  v-model="filterQueries[field.key]"
                  type="search"
                  :aria-label="`筛选${field.label}`"
                  placeholder="输入名称筛选…"
                  class="w-full px-2.5 py-1.5 rounded-lg bg-slate-900 border border-slate-700/80 text-xs text-slate-200 placeholder:text-slate-500 focus:outline-none focus:border-emerald-500"
                />
                <div v-if="filteredOptions(field).length" class="max-h-36 overflow-y-auto space-y-1 pr-1">
                  <label
                    v-for="option in filteredOptions(field)"
                    :key="option.id"
                    class="flex items-center gap-2 px-2 py-1 rounded-lg hover:bg-slate-900/90 cursor-pointer text-xs text-slate-200"
                  >
                    <input
                      type="checkbox"
                      :checked="isIdSelected(field.key, option.id)"
                      @change="toggleId(field.key, option.id)"
                      class="rounded bg-slate-900 border-slate-700 cursor-pointer"
                    />
                    <span class="truncate">{{ option.name }}</span>
                  </label>
                </div>
                <p v-else class="text-xs text-slate-500 px-1 py-2 text-center">未找到匹配「{{ filterQueries[field.key] }}」的选项</p>
              </div>
            </div>

            <!-- 5. 数字 / 日期 / 文本输入 -->
            <input
              v-else
              :id="`rule-${depth}-${field.key}`"
              :type="field.type || 'text'"
              :value="read(field)"
              @input="write(field, ($event.target as HTMLInputElement).value)"
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
            @click="removeField(field.key)"
            class="cursor-pointer self-end sm:self-start sm:mt-1.5 p-1.5 rounded-lg text-slate-400 hover:text-rose-300 hover:bg-rose-500/10 transition-colors shrink-0"
          >
            <X class="w-3.5 h-3.5" aria-hidden="true" />
          </button>
        </div>
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
            v-for="qf in quickFields"
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
            <span>{{ pickerOpen ? '收起条件目录' : `更多条件（${allFields.length} 项）` }}</span>
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

<script setup lang="ts">
import { ref, reactive, computed, watch, nextTick } from 'vue'
import { X, ChevronDown, ChevronUp } from 'lucide-vue-next'
import {
  COUNTRY_OPTIONS,
  LANGUAGE_OPTIONS,
  CATEGORY_OPTIONS,
  GENRE_OPTIONS,
  NETWORK_PRESET_OPTIONS,
  CERTIFICATION_OPTIONS,
  VERSION_TAG_OPTIONS
} from '../api/tmdbMappings'

const rule = defineModel<any>({ required: true })
const props = defineProps<{ options: any; depth?: number }>()
const depth = props.depth || 0

const filterQueries = reactive<Record<string, string>>({})
const customInputs = reactive<Record<string, string>>({})
const pickerOpen = ref(false)
const pickerSearch = ref('')
const addedKeys = ref<string[]>([])

const choices = (pairs: string[][]) => pairs.map(([value, label]) => ({ value, label }))

const sections: any[] = [
  {
    title: '基本筛选',
    fields: [
      { key: 'media_type', label: '媒体类型', shortLabel: '媒体类型', options: choices([['movie', '仅电影'], ['tv', '仅剧集']]) },
      { key: 'categories', label: '基础分类', shortLabel: '分类', presetOptions: CATEGORY_OPTIONS },
      { key: 'countries', label: '国家或地区', shortLabel: '地区', presetOptions: COUNTRY_OPTIONS, allowCustom: true, customPlaceholder: '其他代码如 SE' },
      { key: 'genres', label: '流派', shortLabel: '流派', presetOptions: GENRE_OPTIONS, allowCustom: true, customPlaceholder: '其他流派名' },
      { key: 'genres_all', label: '流派全匹配', type: 'checkbox', checkboxLabel: '所选流派必须全部满足' },
      { key: 'min_rating', label: '最低评分', shortLabel: '最低评分', type: 'number', step: 0.1, min: 0, max: 10 },
      { key: 'max_rating', label: '最高评分', type: 'number', step: 0.1, min: 0, max: 10 },
      { key: 'year_min', label: '最早发行年份', shortLabel: '发行年份≥', type: 'number', min: 0 },
      { key: 'year_max', label: '最晚发行年份', type: 'number', min: 0 },
      { key: 'networks', label: '播出网络名称', shortLabel: '播出网络', presetOptions: NETWORK_PRESET_OPTIONS, allowCustom: true, customPlaceholder: '其他网络如 BBC' }
    ]
  },
  {
    title: '语言、口碑与时长',
    fields: [
      { key: 'languages', label: '原始语言', shortLabel: '原始语言', presetOptions: LANGUAGE_OPTIONS, allowCustom: true, customPlaceholder: 'ISO代码如 pl', help: '作品原作语言（按 TMDB original_language 匹配）；本地配音请使用「资源标签」。' },
      { key: 'spoken_languages', label: '对白语言', shortLabel: '对白语言', presetOptions: LANGUAGE_OPTIONS, allowCustom: true, customPlaceholder: 'ISO代码如 pl' },
      { key: 'min_votes', label: '最低投票人数', type: 'number', min: 0 },
      { key: 'runtime_min', label: '最短时长（分钟）', type: 'number', min: 0 },
      { key: 'runtime_max', label: '最长时长（分钟）', type: 'number', min: 0, help: '剧集按 TMDB 提供的单集常用时长筛选。' },
      { key: 'min_popularity', label: '最低热度', type: 'number', step: 0.1, min: 0 },
      { key: 'sort_by', label: '默认结果排序（降序）', options: choices([['PremiereDate', '发行年份'], ['CommunityRating', '评分'], ['Popularity', '热度'], ['DateCreated', '入库时间'], ['SortName', '片名']]) }
    ]
  },
  {
    title: '剧集状态、播出与本地收齐',
    fields: [
      { key: 'statuses', label: '剧集状态', singleList: true, options: choices([['Ended', '已完结'], ['Returning Series', '持续更新'], ['Canceled', '已取消'], ['In Production', '制作中'], ['Planned', '计划中']]) },
      { key: 'tv_types', label: '节目类型', singleList: true, options: choices([['Miniseries', '迷你剧'], ['Scripted', '剧情类'], ['Reality', '真人秀'], ['Documentary', '纪录片'], ['Talk Show', '访谈'], ['Animation', '动画']]) },
      { key: 'seasons_min', label: '最少季数', type: 'number', min: 0 },
      { key: 'seasons_max', label: '最多季数', type: 'number', min: 0 },
      { key: 'episodes_min', label: '最少总集数', type: 'number', min: 0 },
      { key: 'episodes_max', label: '最多总集数', type: 'number', min: 0, help: '按 TMDB 总集数筛选；如需限制本地已收齐，请使用「本地收齐状态」。' },
      { key: 'local_complete', label: '本地收齐状态', options: choices([['aired', '已播出集已收齐（不含特别篇）'], ['all', '全剧已完结且收齐']]) },
      { key: 'aired_within_days', label: '最近多少天有新集播出', type: 'number', min: 1, max: 36500 },
      { key: 'airing_within_days', label: '未来多少天有播出计划', type: 'number', min: 0, max: 365, help: '0 表示今日即将播出，7 表示未来 7 天。' }
    ]
  },
  {
    title: '制作公司、播出网络、关键词与演职员',
    fields: [
      { key: 'network_ids', label: '播出网络（已刮削）', source: 'networks', help: '从已刮削 TMDB 数据中勾选，支持搜索筛选。' },
      { key: 'company_ids', label: '制作公司', source: 'companies' },
      { key: 'collection_ids', label: '电影合集', source: 'collections' },
      { key: 'has_collection', label: '是否属于电影合集', boolean: true, options: choices([['true', '属于某个合集'], ['false', '未归入合集']]) },
      { key: 'keyword_ids', label: '题材关键词', source: 'keywords' },
      { key: 'director_ids', label: '导演 / 创作者', source: 'directors' },
      { key: 'person_ids', label: '演员 / 导演 / 创作者', source: 'cast' }
    ]
  },
  {
    title: '地区播放平台与分级发行（JustWatch / TMDB）',
    fields: [
      { key: 'watch_region', label: '播放平台地区', options: COUNTRY_OPTIONS },
      { key: 'watch_types', label: '观看方式', singleList: true, options: choices([['flatrate', '订阅流媒体'], ['rent', '租赁'], ['buy', '购买']]) },
      { key: 'watch_provider_ids', label: '可观看平台', source: 'watch_providers', help: '平台数据源自 JustWatch（TMDB Watch Providers）。' },
      { key: 'cert_region', label: '分级 / 发行地区', options: COUNTRY_OPTIONS },
      { key: 'certifications', label: '内容分级', shortLabel: '分级', presetOptions: CERTIFICATION_OPTIONS, allowCustom: true, customPlaceholder: '其他分级如 18' },
      { key: 'digital_within_days', label: '最近多少天数字发行（电影）', type: 'number', min: 1, max: 36500 }
    ]
  },
  {
    title: '日期、关键词与资源版本',
    fields: [
      { key: 'premiered_after', label: '首播日期从', type: 'date' },
      { key: 'premiered_before', label: '首播日期至', type: 'date' },
      { key: 'premiered_within_days', label: '最近多少天首播', type: 'number', min: 1, max: 36500 },
      { key: 'added_within_days', label: '最近多少天入库', type: 'number', min: 1, max: 36500 },
      { key: 'keyword', label: '片名或路径包含', shortLabel: '关键词', placeholder: '输入片名或路径关键词' },
      { key: 'search_overview', label: '检索剧情简介', type: 'checkbox', checkboxLabel: '关键词同时检索剧情简介' },
      { key: 'version_tags', label: '资源版本标签', shortLabel: '资源标签', presetOptions: VERSION_TAG_OPTIONS, allowCustom: true, customPlaceholder: '其他标签回车添加', help: '同一物理文件须同时包含所选标签；剧集存在符合条件的单集即可匹配。' },
      { key: 'exclude_countries', label: '排除国家或地区', shortLabel: '排除地区', presetOptions: COUNTRY_OPTIONS, allowCustom: true, customPlaceholder: '其他代码如 SE' },
      { key: 'exclude_genres', label: '排除流派', shortLabel: '排除流派', presetOptions: GENRE_OPTIONS, allowCustom: true, customPlaceholder: '其他流派名' },
      { key: 'exclude_networks', label: '排除播出网络', shortLabel: '排除网络', presetOptions: NETWORK_PRESET_OPTIONS, allowCustom: true, customPlaceholder: '其他网络名' }
    ]
  }
]

const allFields = sections.flatMap(s => s.fields)
const fieldMap = new Map(allFields.map(f => [f.key, f]))

const quickKeys = ['media_type', 'countries', 'genres', 'min_rating', 'year_min', 'networks', 'keyword', 'version_tags']
const quickFields = quickKeys.map(k => fieldMap.get(k)).filter(Boolean)

function hasValue(field: any, val: any) {
  if (field.type === 'checkbox') return val === true
  if (Array.isArray(val)) return val.length > 0
  return val !== null && val !== undefined && val !== ''
}

watch(() => rule.value, (newVal, oldVal) => {
  const currentRule = newVal || {}
  const next: string[] = []
  if (newVal === oldVal) {
    for (const k of addedKeys.value) {
      if (fieldMap.has(k)) next.push(k)
    }
  }
  for (const field of allFields) {
    if (hasValue(field, currentRule[field.key]) && !next.includes(field.key)) {
      next.push(field.key)
    }
  }
  addedKeys.value = next
}, { immediate: true })

const visibleFields = computed(() =>
  addedKeys.value.map(k => fieldMap.get(k)).filter(Boolean)
)

const activeCount = computed(() => {
  const currentRule = rule.value || {}
  return allFields.filter(f => hasValue(f, currentRule[f.key])).length
})

const hasAnyEmptySource = computed(() =>
  allFields.some(f => isSourceEmpty(f))
)

const filteredSections = computed(() => {
  const q = pickerSearch.value.trim().toLowerCase()
  if (!q) return sections
  return sections
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
  for (const field of allFields) {
    if (rule.value && field.key in rule.value) {
      delete rule.value[field.key]
    }
  }
  addedKeys.value = []
}

function isSourceEmpty(field: any) {
  if (!field.source) return false
  return mergedOptions(field).length === 0
}

function presetDisplayLabel(field: any, val: string) {
  const found = (field.presetOptions || []).find((o: any) => o.value.toLowerCase() === String(val).toLowerCase())
  return found ? found.label : val
}

function availablePresetOptions(field: any) {
  const selected = new Set((rule.value[field.key] || []).map((v: string) => String(v).toLowerCase()))
  return (field.presetOptions || []).filter((o: any) => !selected.has(o.value.toLowerCase()))
}

function addPresetValue(key: string, val: string) {
  if (!val) return
  const current = Array.isArray(rule.value[key]) ? [...rule.value[key]] : []
  if (!current.some(v => String(v).toLowerCase() === val.toLowerCase())) {
    current.push(val)
    rule.value[key] = current
  }
}

function removePresetValue(key: string, val: string) {
  const current = Array.isArray(rule.value[key]) ? [...rule.value[key]] : []
  rule.value[key] = current.filter(v => v !== val)
}

function commitCustomValue(key: string) {
  const raw = (customInputs[key] || '').trim()
  if (!raw) return
  for (const part of raw.split(/[,，]/).map(s => s.trim()).filter(Boolean)) {
    addPresetValue(key, part)
  }
  customInputs[key] = ''
}

function read(field: any) {
  const value = rule.value[field.key]
  if (field.singleList) return value?.[0] || ''
  if (field.boolean) return value == null ? '' : String(value)
  return value ?? ''
}

function write(field: any, value: string) {
  rule.value[field.key] = field.singleList
    ? (value ? [value] : [])
    : field.boolean
      ? (value === '' ? null : value === 'true')
      : field.type === 'number'
        ? (value === '' ? null : Number(value))
        : (value || null)
}

function writeCheckbox(key: string, checked: boolean) {
  if (checked) rule.value[key] = true
  else delete rule.value[key]
}

function mergedOptions(field: any) {
  const values = [...(props.options?.[field.source] || [])]
  for (const id of rule.value[field.key] || []) {
    if (!values.some(v => v.id === id)) values.push({ id, name: `TMDB #${id}` })
  }
  return values
}

function filteredOptions(field: any) {
  const query = (filterQueries[field.key] || '').trim().toLowerCase()
  const selected = new Set(rule.value[field.key] || [])
  const list = mergedOptions(field)
  const filtered = query ? list.filter(o => String(o.name).toLowerCase().includes(query)) : list
  return [...filtered].sort((a, b) => Number(selected.has(b.id)) - Number(selected.has(a.id)))
}

function isIdSelected(key: string, id: number) {
  return (rule.value[key] || []).includes(id)
}

function toggleId(key: string, id: number) {
  const current = Array.isArray(rule.value[key]) ? [...rule.value[key]] : []
  const idx = current.indexOf(id)
  if (idx >= 0) current.splice(idx, 1)
  else current.push(id)
  rule.value[key] = current
}

function addGroup() {
  rule.value.any ||= []
  rule.value.any.push({})
}
</script>

<style scoped>
.rule-input { width: 100%; padding: .5rem .7rem; border: 1px solid #334155; border-radius: .65rem; background: #020617; color: #e2e8f0; min-height: 36px; font-size: 0.75rem; }
.rule-input:focus { outline: 2px solid #10b981; outline-offset: 2px; }
</style>
