<script setup>
import { CheckCircle2, XCircle, AlertTriangle, Info } from 'lucide-vue-next'
import { toastState } from '../../composables/useToast'
</script>

<template>
  <div
    role="status"
    aria-live="polite"
    class="fixed top-5 right-5 z-50 space-y-2.5 max-w-sm w-full pointer-events-none"
  >
    <div
      v-for="t in toastState.items"
      :key="t.id"
      class="pointer-events-auto flex items-start gap-3 px-4 py-3 rounded-xl shadow-2xl border backdrop-blur-md text-sm transition-colors"
      :class="{
        'bg-emerald-950/95 border-emerald-500/40 text-emerald-100': t.type === 'success',
        'bg-rose-950/95 border-rose-500/40 text-rose-100': t.type === 'error',
        'bg-amber-950/95 border-amber-500/40 text-amber-100': t.type === 'warn',
        'bg-slate-900/95 border-slate-700 text-slate-100': t.type === 'info'
      }"
    >
      <CheckCircle2 v-if="t.type === 'success'" class="w-4 h-4 text-emerald-400 shrink-0 mt-0.5" aria-hidden="true" />
      <XCircle v-else-if="t.type === 'error'" class="w-4 h-4 text-rose-400 shrink-0 mt-0.5" aria-hidden="true" />
      <AlertTriangle v-else-if="t.type === 'warn'" class="w-4 h-4 text-amber-400 shrink-0 mt-0.5" aria-hidden="true" />
      <Info v-else class="w-4 h-4 text-sky-400 shrink-0 mt-0.5" aria-hidden="true" />
      <div class="flex-1 leading-snug break-all">{{ t.message }}</div>
    </div>
  </div>
</template>
