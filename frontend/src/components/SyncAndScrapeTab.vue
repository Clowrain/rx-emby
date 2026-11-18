<template>
  <section class="space-y-6">
    <ConfigGroupPanel
      prefix="sync."
      title="strm.txt 同步策略与源过滤"
      description="配置 strm.txt 索引源路径、定时同步周期、目录白名单前缀、排除关键词与调试行数上限，保存后立即生效。如需触发同步或查看任务进度与记录，请前往「任务中心」。"
      :icon="SlidersHorizontal"
      @saved="$emit('saved')"
    />
  </section>
</template>

<script setup>
import { SlidersHorizontal } from 'lucide-vue-next'
import ConfigGroupPanel from './ConfigGroupPanel.vue'

defineEmits(['saved'])
</script>
