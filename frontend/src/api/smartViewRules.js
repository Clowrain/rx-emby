export function validateSmartRule(rule, depth = 0) {
  if (depth > 2 || (rule.any?.length || 0) > 8) throw new Error('条件分组最多 3 层，每层最多 8 组')
  for (const key of ['min_rating', 'max_rating']) {
    const n = rule[key]
    if (n != null && (!Number.isFinite(n) || n < 0 || n > 10)) throw new Error('评分必须在 0～10 之间')
  }
  for (const [min, max, name] of [
    ['year_min','year_max','年份'], ['min_rating','max_rating','评分'],
    ['runtime_min','runtime_max','时长'], ['seasons_min','seasons_max','季数'], ['episodes_min','episodes_max','集数']
  ]) {
    if (rule[min] != null && rule[max] != null && rule[min] > rule[max]) throw new Error(`${name}下限不能大于上限`)
  }
  for (const key of ['year_min','year_max','min_votes','runtime_min','runtime_max','seasons_min','seasons_max','episodes_min','episodes_max','premiered_within_days','added_within_days','digital_within_days','airing_within_days','aired_within_days']) {
    const n = rule[key]
    const minVal = key === 'airing_within_days' ? 0 : (key.endsWith('_days') ? 1 : 0)
    if (n != null && (!Number.isInteger(n) || n < minVal)) throw new Error('年份、人数、时长、季集数和天数须填写有效整数')
  }
  if (rule.premiered_after && rule.premiered_before && rule.premiered_after > rule.premiered_before) throw new Error('开始日期不能晚于结束日期')
  for (const child of rule.any || []) validateSmartRule(child, depth + 1)
  return rule
}
