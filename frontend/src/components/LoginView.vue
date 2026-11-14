<template>
  <div class="min-h-dvh flex items-center justify-center bg-[#060911] text-slate-100 px-4 py-10 relative overflow-hidden">
    <!-- 背景氛围光晕 -->
    <div
      class="pointer-events-none absolute -top-40 left-1/2 -translate-x-1/2 w-[560px] h-[360px] rounded-full bg-emerald-500/10 blur-3xl"
      aria-hidden="true"
    />

    <main class="w-full max-w-md relative z-10">
      <section
        aria-labelledby="login-heading"
        class="bg-[#0b101d]/95 border border-slate-800/80 rounded-2xl shadow-2xl shadow-black/60 p-8 space-y-6 backdrop-blur-md"
      >
        <!-- 品牌标题区 -->
        <div class="flex items-center gap-3.5">
          <div class="w-11 h-11 rounded-xl bg-gradient-to-br from-emerald-500 to-cyan-600 flex items-center justify-center shadow-lg shadow-emerald-950/80 ring-1 ring-white/15 shrink-0">
            <Play class="w-5 h-5 text-white fill-current ml-0.5" aria-hidden="true" />
          </div>
          <div>
            <div class="flex items-center gap-2">
              <h1 id="login-heading" class="font-bold text-xl tracking-tight text-white">
                RX-Emby 管理后台
              </h1>
              <span class="px-1.5 py-0.5 text-[10px] font-semibold font-mono rounded bg-emerald-500/15 text-emerald-400 border border-emerald-500/30">
                PRO
              </span>
            </div>
            <p class="text-xs text-slate-400 mt-0.5">
              仅限系统管理员权限角色登录访问
            </p>
          </div>
        </div>

        <!-- 权限提示 / 错误反馈 -->
        <div
          v-if="displayError"
          role="alert"
          aria-live="polite"
          class="flex items-start gap-2.5 px-3.5 py-3 rounded-xl bg-rose-950/70 border border-rose-500/40 text-rose-200 text-xs leading-relaxed"
        >
          <AlertCircle class="w-4 h-4 text-rose-400 shrink-0 mt-0.5" aria-hidden="true" />
          <span>{{ displayError }}</span>
        </div>

        <!-- 登录表单 -->
        <form method="post" class="space-y-4" @submit.prevent="handleSubmit">
          <div class="space-y-1.5">
            <label for="admin-username" class="block text-xs font-medium text-slate-300">
              管理员用户名 <span class="text-rose-400" aria-hidden="true">*</span>
            </label>
            <input
              ref="usernameInputRef"
              id="admin-username"
              v-model="username"
              name="username"
              type="text"
              autocomplete="username"
              autocorrect="off"
              autocapitalize="none"
              spellcheck="false"
              enterkeyhint="next"
              required
              placeholder="请输入管理员账号（默认 admin）"
              :aria-invalid="Boolean(errorMsg)"
              class="w-full min-h-12 px-3.5 py-2.5 rounded-xl bg-slate-950/90 border border-slate-700/80 text-base sm:text-sm text-white placeholder:text-slate-500 focus-visible:outline-2 focus-visible:outline-emerald-400 focus-visible:outline-offset-2 focus:border-emerald-500 transition-colors"
              @input="clearError"
            />
          </div>

          <div class="space-y-1.5">
            <label for="admin-password" class="block text-xs font-medium text-slate-300">
              登录密码
            </label>
            <div class="relative">
              <input
                id="admin-password"
                v-model="password"
                name="password"
                :type="showPassword ? 'text' : 'password'"
                autocomplete="current-password"
                enterkeyhint="done"
                placeholder="若未设置初始密码可留空"
                :aria-invalid="Boolean(errorMsg)"
                class="w-full min-h-12 pl-3.5 pr-11 py-2.5 rounded-xl bg-slate-950/90 border border-slate-700/80 text-base sm:text-sm text-white placeholder:text-slate-500 focus-visible:outline-2 focus-visible:outline-emerald-400 focus-visible:outline-offset-2 focus:border-emerald-500 transition-colors"
                @input="clearError"
              />
              <button
                type="button"
                :aria-pressed="showPassword"
                :aria-label="showPassword ? '隐藏密码' : '显示密码'"
                @click="showPassword = !showPassword"
                class="cursor-pointer absolute right-2 top-1/2 -translate-y-1/2 p-2 rounded-lg text-slate-400 hover:text-slate-200 hover:bg-slate-800/70 focus-visible:outline-2 focus-visible:outline-emerald-400 transition-colors"
              >
                <EyeOff v-if="showPassword" class="w-4 h-4" aria-hidden="true" />
                <Eye v-else class="w-4 h-4" aria-hidden="true" />
              </button>
            </div>
          </div>

          <button
            type="submit"
            :disabled="submitting"
            class="cursor-pointer w-full min-h-12 mt-2 px-4 py-2.5 rounded-xl bg-gradient-to-r from-emerald-600 to-cyan-600 hover:from-emerald-500 hover:to-cyan-500 disabled:opacity-60 disabled:cursor-not-allowed text-sm font-semibold text-white shadow-lg shadow-emerald-950/60 focus-visible:outline-2 focus-visible:outline-emerald-400 focus-visible:outline-offset-2 transition-all flex items-center justify-center gap-2"
          >
            <ShieldCheck v-if="!submitting" class="w-4 h-4" aria-hidden="true" />
            <span>{{ submitting ? '正在验证权限…' : '登录管理后台' }}</span>
          </button>
        </form>

        <div class="pt-3 border-t border-slate-800/70 flex items-center justify-between text-[11px] text-slate-500">
          <span class="flex items-center gap-1.5">
            <Lock class="w-3.5 h-3.5 text-emerald-500/80" aria-hidden="true" />
            <span>系统管理员身份校验已启用</span>
          </span>
          <span>普通用户仅限客户端播放</span>
        </div>
      </section>
    </main>
  </div>
</template>

<script setup>
import { ref, computed } from 'vue'
import { Play, ShieldCheck, Eye, EyeOff, AlertCircle, Lock } from 'lucide-vue-next'
import { adminApi, setAdminToken } from '../api/admin'

const props = defineProps({
  notice: {
    type: String,
    default: ''
  }
})

const emit = defineEmits(['logged-in'])

const username = ref('')
const password = ref('')
const showPassword = ref(false)
const submitting = ref(false)
const errorMsg = ref('')
const usernameInputRef = ref(null)

const displayError = computed(() => errorMsg.value || props.notice)

function clearError() {
  if (errorMsg.value) {
    errorMsg.value = ''
  }
}

async function handleSubmit() {
  const trimmedUser = username.value.trim()
  if (!trimmedUser) {
    errorMsg.value = '请输入管理员用户名'
    usernameInputRef.value?.focus()
    return
  }

  submitting.value = true
  errorMsg.value = ''
  try {
    const res = await adminApi.login(trimmedUser, password.value)
    setAdminToken(res.token)
    emit('logged-in', res.user)
  } catch (err) {
    errorMsg.value = err?.message || '登录失败，请检查账号密码或管理员权限'
  } finally {
    submitting.value = false
  }
}
</script>
