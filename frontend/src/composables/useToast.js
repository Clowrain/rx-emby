import { reactive } from 'vue'

export const toastState = reactive({
  items: []
})

export const confirmState = reactive({
  visible: false,
  title: '',
  message: '',
  confirmText: '确认执行',
  cancelText: '取消',
  danger: false,
  resolve: null
})

let nextId = 1

export function useToast() {
  function push(message, type = 'success', duration = 3200) {
    const id = nextId++
    toastState.items.push({ id, message, type })
    setTimeout(() => {
      const idx = toastState.items.findIndex(t => t.id === id)
      if (idx !== -1) toastState.items.splice(idx, 1)
    }, duration)
  }

  return {
    success: (msg, dur) => push(msg, 'success', dur),
    error: (msg, dur) => push(msg, 'error', dur || 4500),
    info: (msg, dur) => push(msg, 'info', dur),
    warn: (msg, dur) => push(msg, 'warn', dur)
  }
}

export function useConfirm() {
  return function confirmModal({
    title = '确认操作',
    message = '是否确认继续执行该操作？',
    confirmText = '确认',
    cancelText = '取消',
    danger = false
  } = {}) {
    return new Promise(resolve => {
      confirmState.title = title
      confirmState.message = message
      confirmState.confirmText = confirmText
      confirmState.cancelText = cancelText
      confirmState.danger = danger
      confirmState.resolve = resolve
      confirmState.visible = true
    })
  }
}

export function resolveConfirm(result) {
  confirmState.visible = false
  if (confirmState.resolve) {
    confirmState.resolve(result)
    confirmState.resolve = null
  }
}
