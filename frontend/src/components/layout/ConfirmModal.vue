<script setup>
import { AlertTriangle, Info } from 'lucide-vue-next'
import { confirmState, resolveConfirm } from '../../composables/useToast'
</script>

<template>
  <div
    v-if="confirmState.visible"
    role="dialog"
    aria-modal="true"
    aria-labelledby="confirm-dialog-title"
    class="fixed inset-0 bg-black/75 backdrop-blur-xs flex items-center justify-center z-50 p-4"
    @click.self="resolveConfirm(false)"
  >
    <div class="card p-6 rounded-2xl max-w-md w-full space-y-4">
      <div class="flex items-start gap-3">
        <div
          class="w-9 h-9 rounded-xl flex items-center justify-center shrink-0"
          :class="confirmState.danger ? 'bg-rose-500/15 text-rose-400 border border-rose-500/30' : 'bg-emerald-500/15 text-emerald-400 border border-emerald-500/30'"
        >
          <AlertTriangle v-if="confirmState.danger" class="w-5 h-5" aria-hidden="true" />
          <Info v-else class="w-5 h-5" aria-hidden="true" />
        </div>
        <div class="space-y-1">
          <h2 id="confirm-dialog-title" class="text-base font-bold text-white">{{ confirmState.title }}</h2>
          <p class="text-sm text-slate-300 leading-relaxed">{{ confirmState.message }}</p>
        </div>
      </div>
      <div class="flex justify-end gap-2.5 pt-2">
        <button
          type="button"
          @click="resolveConfirm(false)"
          class="cursor-pointer px-4 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 text-xs font-medium text-slate-200 transition-colors"
        >
          {{ confirmState.cancelText }}
        </button>
        <button
          type="button"
          @click="resolveConfirm(true)"
          class="cursor-pointer px-4 py-2 rounded-xl text-xs font-semibold text-white transition-colors"
          :class="
            confirmState.danger
              ? 'bg-rose-600 hover:bg-rose-500'
              : 'bg-emerald-600 hover:bg-emerald-500'
          "
        >
          {{ confirmState.confirmText }}
        </button>
      </div>
    </div>
  </div>
</template>
