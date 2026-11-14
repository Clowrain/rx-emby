<script setup>
import { Link2, X } from 'lucide-vue-next'

defineProps({
  modal: { type: Object, required: true },
  binding: { type: Boolean, default: false }
})

const emit = defineEmits(['close', 'submit'])
</script>

<template>
  <div
    v-if="modal.visible"
    role="dialog"
    aria-modal="true"
    aria-labelledby="bind-modal-title"
    class="fixed inset-0 bg-black/75 backdrop-blur-xs flex items-center justify-center z-50 p-4"
    @click.self="emit('close')"
  >
    <div class="card p-6 rounded-2xl max-w-md w-full space-y-4">
      <div class="flex items-center justify-between">
        <h3 id="bind-modal-title" class="text-base font-bold text-white flex items-center gap-2">
          <Link2 class="w-4 h-4 text-emerald-400" aria-hidden="true" />
          <span>手动绑定 TMDB ID</span>
        </h3>
        <button
          type="button"
          aria-label="关闭绑定弹窗"
          @click="emit('close')"
          class="cursor-pointer text-slate-400 hover:text-white p-1 rounded-lg hover:bg-slate-800 transition-colors"
        >
          <X class="w-4 h-4" aria-hidden="true" />
        </button>
      </div>
      <div class="p-3 rounded-xl bg-slate-950/80 border border-slate-800 text-xs font-mono text-slate-400 break-all">
        {{ modal.rootDir }}
      </div>
      <div class="space-y-3">
        <div>
          <label for="bind-tmdb-id" class="block text-xs text-slate-300 mb-1.5">TMDB 数字编号</label>
          <input
            id="bind-tmdb-id"
            v-model="modal.tmdbId"
            type="text"
            inputmode="numeric"
            placeholder="例如：279593"
            class="w-full bg-slate-950 border border-slate-700 focus:border-emerald-500 rounded-xl px-3.5 py-2 text-sm font-mono tabular-nums"
          />
        </div>
        <div>
          <label for="bind-media-type" class="block text-xs text-slate-300 mb-1.5">指定媒体类型（可选）</label>
          <select
            id="bind-media-type"
            v-model="modal.mediaType"
            class="w-full bg-slate-950 border border-slate-700 rounded-xl px-3.5 py-2 text-sm"
          >
            <option value="">自动识别（推荐）</option>
            <option value="movie">电影</option>
            <option value="tv">剧集</option>
          </select>
        </div>
      </div>
      <div class="flex justify-end gap-2.5 pt-2">
        <button
          type="button"
          @click="emit('close')"
          class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-4 py-2 rounded-xl text-xs font-medium transition-colors"
        >
          取消
        </button>
        <button
          type="button"
          @click="emit('submit')"
          :disabled="binding"
          class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 px-4 py-2 rounded-xl text-xs font-semibold text-white transition-colors"
        >
          {{ binding ? '正在拉取 TMDB…' : '确认绑定并锁定' }}
        </button>
      </div>
    </div>
  </div>
</template>
