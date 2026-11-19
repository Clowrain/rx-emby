<script setup lang="ts">
import { ref } from 'vue'
import { Upload, Download, Film, Tv, X, FileArchive } from 'lucide-vue-next'
import { ruleBadges } from '../../utils/smartViewLabels.js'

export interface VirtualLibraryImportModalState {
  visible: boolean
  fileName: string
  rawBytes: Uint8Array | null
  loadingPreview: boolean
  applying: boolean
  pruneMissing: boolean
  error: string
  preview: {
    total_count: number
    create_count: number
    update_count: number
    custom_cover_count: number
    unmatched_existing_count: number
    items: Array<{
      name: string
      action: 'create' | 'update'
      icon_type: string
      sort_order: number
      is_enabled: boolean
      cover_image?: string | null
      cover_type: string
      cover_label: string
      filter_rules: any
    }>
  } | null
}

defineProps<{
  modal: VirtualLibraryImportModalState
  ruleOptions: any
  downloadingBundle?: boolean
}>()

const emit = defineEmits<{
  close: []
  selectFile: [file: File]
  confirm: []
  downloadBundle: []
}>()

const fileInputRef = ref<HTMLInputElement | null>(null)

function triggerFileDialog() {
  if (fileInputRef.value) {
    fileInputRef.value.value = ''
    fileInputRef.value.click()
  }
}

function onFileChange(e: Event) {
  const input = e.target as HTMLInputElement | null
  const file = input?.files?.[0]
  if (file) {
    emit('selectFile', file)
  }
}
</script>

<template>
  <div
    v-if="modal.visible"
    role="dialog"
    aria-modal="true"
    aria-labelledby="vlib-import-modal-title"
    class="fixed inset-0 bg-black/75 backdrop-blur-xs flex items-center justify-center z-50 p-3 sm:p-4"
    @click.self="!modal.applying && emit('close')"
  >
    <div class="card p-4 sm:p-6 rounded-2xl w-full max-w-3xl space-y-4 max-h-[92dvh] overflow-y-auto overscroll-contain">
      <div class="flex items-center justify-between gap-3 border-b border-slate-800 pb-3">
        <h3 id="vlib-import-modal-title" class="text-base font-bold text-white flex items-center gap-2">
          <Upload class="w-4 h-4 text-emerald-400 shrink-0" aria-hidden="true" />
          <span>预览并导入视图配置</span>
        </h3>
        <button
          type="button"
          aria-label="关闭导入视图配置弹窗"
          @click="emit('close')"
          :disabled="modal.applying"
          class="cursor-pointer text-slate-400 hover:text-white p-1 rounded-lg hover:bg-slate-800 transition-colors shrink-0"
        >
          <X class="w-4 h-4" aria-hidden="true" />
        </button>
      </div>

      <!-- 文件选择与 AI 模板快捷下载区 -->
      <div class="rounded-xl border border-dashed border-slate-700 bg-slate-950/60 p-4 space-y-3">
        <input
          ref="fileInputRef"
          type="file"
          accept=".zip,.json,application/zip,application/json"
          class="hidden"
          @change="onFileChange"
        />
        <div class="flex flex-wrap items-center justify-between gap-3">
          <div class="flex items-center gap-3 min-w-0">
            <div class="w-10 h-10 rounded-xl bg-emerald-500/15 border border-emerald-500/30 text-emerald-300 flex items-center justify-center shrink-0">
              <FileArchive class="w-5 h-5" aria-hidden="true" />
            </div>
            <div class="min-w-0">
              <div class="text-xs font-semibold text-slate-200 truncate">
                {{ modal.fileName ? `当前文件：${modal.fileName}` : '选择 .zip 视图配置包或 .json 配置文件' }}
              </div>
              <div class="text-[11px] text-slate-400 mt-0.5">
                支持包含 views.json 与 covers/ 自定义封面的 .zip 包，或 AI 生成的纯 .json 文件
              </div>
            </div>
          </div>
          <button
            type="button"
            @click="triggerFileDialog"
            :disabled="modal.loadingPreview || modal.applying"
            class="cursor-pointer px-3.5 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 border border-slate-700 text-xs font-semibold text-emerald-300 transition-colors shrink-0 disabled:opacity-50"
          >
            {{ modal.fileName ? '重新选择文件' : '选择配置文件' }}
          </button>
        </div>

        <div class="pt-2.5 border-t border-slate-800/80 flex flex-wrap items-center justify-between gap-2 text-[11px] text-slate-400">
          <span>还没有配置文件？可先下载包含 TMDB 字段字典与 18 个参考模板的压缩包，发给 AI 对话生成。</span>
          <button
            type="button"
            @click="emit('downloadBundle')"
            :disabled="downloadingBundle"
            class="cursor-pointer inline-flex items-center gap-1 text-indigo-300 hover:text-indigo-200 font-medium underline underline-offset-4 disabled:opacity-50"
          >
            <Download class="w-3.5 h-3.5" aria-hidden="true" />
            <span>{{ downloadingBundle ? '正在打包…' : '下载当前配置与 AI 编写模板 (.zip)' }}</span>
          </button>
        </div>
      </div>

      <!-- 解析中提示 -->
      <div v-if="modal.loadingPreview" class="py-8 text-center text-xs text-slate-400">
        正在校验视图配置与封面素材…
      </div>

      <!-- 错误提示 -->
      <div
        v-if="modal.error"
        role="alert"
        class="rounded-xl border border-rose-500/40 bg-rose-950/30 px-3.5 py-2.5 text-xs text-rose-200"
      >
        {{ modal.error }}
      </div>

      <!-- 预览结果清单 -->
      <div v-if="modal.preview" class="space-y-3">
        <div class="flex flex-wrap items-center justify-between gap-2 px-1 text-xs">
          <div class="text-slate-200 font-medium">
            已识别 <strong class="text-emerald-300 font-mono tabular-nums">{{ modal.preview.total_count }}</strong> 项视图配置（新增
            <strong class="text-emerald-300 font-mono tabular-nums">{{ modal.preview.create_count }}</strong> 项，同名更新
            <strong class="text-amber-300 font-mono tabular-nums">{{ modal.preview.update_count }}</strong> 项<template v-if="modal.preview.custom_cover_count > 0">，含
            <strong class="text-indigo-300 font-mono tabular-nums">{{ modal.preview.custom_cover_count }}</strong> 张自定义封面</template>）
          </div>
        </div>

        <div class="max-h-72 overflow-y-auto divide-y divide-slate-800/80 rounded-xl border border-slate-800 bg-slate-950/70">
          <div
            v-for="(item, idx) in modal.preview.items"
            :key="item.name"
            class="p-3 flex flex-col gap-1.5 text-xs"
          >
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div class="flex items-center gap-2 min-w-0">
                <span class="text-[11px] font-mono text-slate-500 tabular-nums">#{{ idx + 1 }}</span>
                <Film v-if="item.icon_type === 'movies'" class="w-3.5 h-3.5 text-emerald-400 shrink-0" aria-hidden="true" />
                <Tv v-else class="w-3.5 h-3.5 text-emerald-400 shrink-0" aria-hidden="true" />
                <span class="font-bold text-white truncate">{{ item.name }}</span>
                <span
                  class="px-2 py-0.5 rounded-md text-[10px] font-semibold border shrink-0"
                  :class="
                    item.action === 'create'
                      ? 'bg-emerald-500/15 border-emerald-500/40 text-emerald-300'
                      : 'bg-amber-500/15 border-amber-500/40 text-amber-300'
                  "
                >
                  {{ item.action === 'create' ? '新增' : '同名更新' }}
                </span>
              </div>
              <div class="flex items-center gap-1.5 text-[11px] shrink-0">
                <span class="px-2 py-0.5 rounded bg-slate-900 border border-slate-800 text-slate-300">
                  {{ item.cover_label }}
                </span>
                <span class="px-2 py-0.5 rounded bg-slate-900 border border-slate-800 text-slate-400 font-mono tabular-nums">
                  排序 #{{ item.sort_order }}
                </span>
              </div>
            </div>
            <div class="text-[11px] text-slate-400 leading-relaxed">
              {{ ruleBadges(item.filter_rules, ruleOptions).join(' · ') || '无附加过滤条件（匹配全库）' }}
            </div>
          </div>
        </div>

        <!-- 清理未包含的旧视图开关 -->
        <div
          v-if="modal.preview.unmatched_existing_count > 0"
          class="rounded-xl border border-amber-500/30 bg-amber-950/20 px-3.5 py-2.5 space-y-1"
        >
          <label class="flex items-center gap-2 text-xs font-medium text-amber-200 cursor-pointer">
            <input
              v-model="modal.pruneMissing"
              type="checkbox"
              class="rounded bg-slate-900 border-slate-700 cursor-pointer"
            />
            <span>同时移除本次配置中未包含的 {{ modal.preview.unmatched_existing_count }} 个旧视图</span>
          </label>
          <p class="text-[11px] text-slate-400 pl-6">
            默认仅新增或更新同名视图（保留原有视图 ID 与用户访问权限）；勾选后将彻底删除其余旧视图。
          </p>
        </div>
      </div>

      <div class="flex justify-end gap-2.5 pt-3 border-t border-slate-800">
        <button
          type="button"
          @click="emit('close')"
          :disabled="modal.applying"
          class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-4 py-2 rounded-xl text-xs font-medium transition-colors"
        >
          取消
        </button>
        <button
          type="button"
          @click="emit('confirm')"
          :disabled="!modal.preview || modal.applying || modal.loadingPreview"
          class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 px-5 py-2 rounded-xl text-xs font-semibold text-white transition-colors disabled:opacity-50"
        >
          {{
            modal.applying
              ? '正在应用配置…'
              : modal.preview
                ? `确认应用配置（${modal.preview.total_count} 项）`
                : '确认应用配置'
          }}
        </button>
      </div>
    </div>
  </div>
</template>
