import { ref, reactive, computed, watch } from 'vue'
import { adminApi } from '../api/admin.js'
import { useToast, useConfirm } from './useToast.js'
import { validateSmartRule } from '../api/smartViewRules.js'
import { ruleBadges } from '../utils/smartViewLabels.js'

export function useVirtualLibraries(deps = {}) {
  const api = deps.api ?? adminApi
  const toast = deps.toast ?? useToast()
  const confirmModal = deps.confirmModal ?? useConfirm()
  const validateRule = deps.validateRule ?? validateSmartRule

  const loading = ref(false)
  const saving = ref(false)
  const seeding = ref(false)
  const backfilling = ref(false)
  const previewing = ref(false)
  const togglingId = ref(null)
  const deletingId = ref(null)
  const listFilter = ref('all')
  const listQuery = ref('')
  const previewCount = ref(null)
  const formError = ref('')
  const ruleOptions = ref({ networks: [], companies: [], collections: [], metadata: {} })
  const libraries = ref([])
  const coverBust = ref(Date.now())

  const modal = reactive({
    visible: false,
    mode: 'create',
    step: 1,
    id: null,
    name: '',
    iconType: 'tvshows',
    coverImage: '',
    sortOrder: 10,
    isEnabled: true,
    rules: {}
  })

  const filterTabs = computed(() => {
    const total = libraries.value.length
    const enabled = libraries.value.filter(v => v.is_enabled).length
    return [
      { key: 'all', label: '全部', count: total },
      { key: 'enabled', label: '首页已挂载', count: enabled },
      { key: 'disabled', label: '已停用', count: total - enabled }
    ]
  })

  const filteredLibraries = computed(() => {
    const q = listQuery.value.trim().toLowerCase()
    return libraries.value.filter(v => {
      if (listFilter.value === 'enabled' && !v.is_enabled) return false
      if (listFilter.value === 'disabled' && v.is_enabled) return false
      if (!q) return true
      const nameMatch = String(v.name || '').toLowerCase().includes(q)
      const badgeMatch = ruleBadges(v.filter_rules, ruleOptions.value).some(b =>
        b.toLowerCase().includes(q)
      )
      return nameMatch || badgeMatch
    })
  })

  const metadataPercent = computed(() => {
    const total = Number(ruleOptions.value?.metadata?.total || 0)
    const enriched = Number(ruleOptions.value?.metadata?.enriched || 0)
    if (total <= 0) return 0
    return Math.min(100, Math.round((enriched / total) * 100))
  })

  watch(() => modal.rules, () => {
    previewCount.value = null
  }, { deep: true })

  async function loadLibraries() {
    loading.value = true
    try {
      const data = await api.listVirtualLibraries()
      libraries.value = data.libraries || []
    } catch (err) {
      toast.error('加载智能视图失败：' + err.message)
    } finally {
      loading.value = false
    }
  }

  async function loadRuleOptions() {
    try {
      ruleOptions.value = await api.smartViewOptions()
    } catch (err) {
      toast.error('加载筛选选项失败：' + err.message)
    }
  }

  function openModal(item = null, mode = 'create', step = 1) {
    const r = item ? item.filter_rules || {} : {}
    modal.mode = mode
    modal.step = step
    modal.id = item ? item.id : null
    modal.name = item ? item.name : ''
    modal.iconType = item ? item.icon_type : 'tvshows'
    modal.coverImage = item ? (item.cover_image || '') : ''
    modal.sortOrder = item ? item.sort_order : 10
    modal.isEnabled = item ? item.is_enabled : true
    modal.rules = JSON.parse(JSON.stringify(r))
    formError.value = ''
    previewCount.value = null
    modal.visible = true
  }

  function openInfoModal(item) {
    openModal(item, 'info', 1)
  }

  function openRulesModal(item) {
    openModal(item, 'rules', 2)
  }

  function openCreateModal() {
    openModal(null, 'create', 1)
  }

  function validateBasicInfo() {
    if (!modal.name.trim() || Array.from(modal.name.trim()).length > 64) {
      throw new Error('视图名称须为 1～64 个字符')
    }
    if (!Number.isInteger(Number(modal.sortOrder))) {
      throw new Error('排序权重须为整数')
    }
  }

  function nextCreateStep() {
    formError.value = ''
    try {
      validateBasicInfo()
    } catch (err) {
      formError.value = err.message
      return
    }
    modal.step = 2
  }

  async function previewRules() {
    if (previewing.value) return
    previewing.value = true
    formError.value = ''
    const snapshot = JSON.stringify(modal.rules)
    try {
      validateRule(modal.rules)
      const result = await api.previewSmartView(modal.rules)
      if (snapshot === JSON.stringify(modal.rules)) {
        previewCount.value = result.matched_count
      }
    } catch (err) {
      formError.value = err.message
    } finally {
      previewing.value = false
    }
  }

  async function submitModal() {
    if (saving.value) return
    formError.value = ''
    try {
      validateBasicInfo()
      validateRule(modal.rules)
    } catch (err) {
      formError.value = err.message
      return
    }
    saving.value = true
    try {
      const res = await api.saveVirtualLibrary({
        id: modal.id,
        name: modal.name.trim(),
        icon_type: modal.iconType,
        cover_image: modal.coverImage || null,
        sort_order: Number(modal.sortOrder) || 0,
        is_enabled: modal.isEnabled,
        filter_rules: JSON.parse(JSON.stringify(modal.rules))
      })
      toast.success(res.message)
      modal.visible = false
      coverBust.value = Date.now()
      await loadLibraries()
    } catch (err) {
      formError.value = '保存失败：' + err.message
    } finally {
      saving.value = false
    }
  }

  async function backfillMetadata() {
    if (backfilling.value) return
    const ok = await confirmModal({
      title: '补齐 TMDB 扩展信息',
      message: '要为本地已刮削且未锁定的影视补齐语言、投票数、时长等扩展信息吗？任务会分批请求 TMDB，可在关闭全库自动刮削时执行。',
      confirmText: '开始补齐'
    })
    if (!ok) return
    backfilling.value = true
    try {
      const res = await api.backfillSmartViewMetadata()
      toast.success(res.message)
      window.dispatchEvent(new CustomEvent('rx-scrape-task-started'))
      await loadRuleOptions()
    } catch (err) {
      toast.error('补齐失败：' + err.message)
    } finally {
      backfilling.value = false
    }
  }

  async function handleSeedPresets() {
    if (seeding.value) return
    seeding.value = true
    try {
      const data = await api.seedPresetVirtualLibraries()
      toast.success(data.message)
      await loadLibraries()
    } catch (err) {
      toast.error('生成预设失败：' + err.message)
    } finally {
      seeding.value = false
    }
  }

  async function toggleEnabled(v) {
    if (togglingId.value != null) return
    togglingId.value = v.id
    const nextEnabled = !v.is_enabled
    try {
      await api.saveVirtualLibrary({
        id: v.id,
        name: v.name,
        icon_type: v.icon_type,
        cover_image: v.cover_image || null,
        sort_order: Number(v.sort_order) || 0,
        is_enabled: nextEnabled,
        filter_rules: JSON.parse(JSON.stringify(v.filter_rules || {}))
      })
      v.is_enabled = nextEnabled
      toast.success(nextEnabled ? `已挂载「${v.name}」至首页` : `已停用「${v.name}」`)
    } catch (err) {
      toast.error('切换状态失败：' + err.message)
    } finally {
      togglingId.value = null
    }
  }

  async function handleDelete(v) {
    if (deletingId.value != null) return
    const ok = await confirmModal({
      title: '删除智能视图',
      message: `确认删除智能视图「${v.name}」吗？该操作不会影响在库影视资源。`,
      confirmText: '确认删除',
      danger: true
    })
    if (!ok) return
    deletingId.value = v.id
    try {
      await api.deleteVirtualLibrary(v.id)
      toast.info(`已删除智能视图「${v.name}」`)
      await loadLibraries()
    } catch (err) {
      toast.error('删除失败：' + err.message)
    } finally {
      deletingId.value = null
    }
  }

  return {
    loading,
    saving,
    seeding,
    backfilling,
    previewing,
    togglingId,
    deletingId,
    listFilter,
    listQuery,
    previewCount,
    formError,
    ruleOptions,
    libraries,
    coverBust,
    modal,
    filterTabs,
    filteredLibraries,
    metadataPercent,
    loadLibraries,
    loadRuleOptions,
    openModal,
    openInfoModal,
    openRulesModal,
    openCreateModal,
    nextCreateStep,
    previewRules,
    submitModal,
    backfillMetadata,
    handleSeedPresets,
    toggleEnabled,
    handleDelete
  }
}
