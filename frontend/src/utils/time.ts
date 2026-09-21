/*
 * 统一时间格式化工具。
 *
 * 后端所有面向用户的时间戳已统一为东八区（+08:00）ISO 字符串（见 backend/src/utils.rs），
 * 但前端若直接用 `new Date(iso).toLocale*` 会落到浏览器所在时区，用户换设备/浏览器时区
 * 设置不同就可能再次错位。这里显式指定 Asia/Shanghai，保证无论客户端时区如何都按
 * 北京时间（UTC+8）展示，与后端语义一致。
 *
 * 一律用 `formatToParts` 拼装，避免依赖具体 locale 的格式差异（如 en-CA 是否输出
 * YYYY-MM-DD、zh-CN 是否带「上午/下午」等），输出结果确定、可预测。
 */

const BEIJING_TIME_ZONE = 'Asia/Shanghai'

function parse(iso: string): Date | null {
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? null : d
}

/** 取某时刻在指定时区下的字段值（补零为两位数）。 */
function parts(
  d: Date,
  fields: Intl.DateTimeFormatOptions,
): Record<'year' | 'month' | 'day' | 'hour' | 'minute' | 'second', string> {
  const fmt = new Intl.DateTimeFormat('en', {
    timeZone: BEIJING_TIME_ZONE,
    hour12: false,
    ...fields,
  })
  const map: Record<string, string> = {}
  for (const part of fmt.formatToParts(d)) {
    if (part.type !== 'literal') map[part.type] = part.value
  }
  const two = (v?: string) => (v ? v.padStart(2, '0') : '00')
  return {
    year: map.year ?? '0000',
    month: two(map.month),
    day: two(map.day),
    hour: two(map.hour),
    minute: two(map.minute),
    second: two(map.second),
  }
}

const DATE_FIELDS: Intl.DateTimeFormatOptions = {
  year: 'numeric',
  month: '2-digit',
  day: '2-digit',
}
const TIME_FIELDS: Intl.DateTimeFormatOptions = {
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
}

/** 东八区 YYYY-MM-DD（用于「是否今天」判断）。 */
function beijingDateKey(d: Date): string {
  const p = parts(d, DATE_FIELDS)
  return `${p.year}-${p.month}-${p.day}`
}

/** 东八区 HH:mm。 */
function beijingHm(d: Date): string {
  const p = parts(d, TIME_FIELDS)
  return `${p.hour}:${p.minute}`
}

/** 今天（北京时间）的 YYYY-MM-DD。 */
function beijingTodayKey(): string {
  return beijingDateKey(new Date())
}

/** 东八区 24 小时制 HH:mm:ss；非法输入原样返回。 */
export function formatTimeHms(iso: string): string {
  const d = parse(iso)
  if (!d) return iso
  const p = parts(d, TIME_FIELDS)
  return `${p.hour}:${p.minute}:${p.second}`
}

/** 东八区完整日期时间 YYYY-MM-DD HH:mm:ss；非法输入原样返回。 */
export function formatDateTime(iso: string): string {
  const d = parse(iso)
  if (!d) return iso
  const p = parts(d, { ...DATE_FIELDS, ...TIME_FIELDS })
  return `${p.year}-${p.month}-${p.day} ${p.hour}:${p.minute}:${p.second}`
}

/** 当天显示 HH:mm，跨天显示 MM-DD HH:mm（均北京时间）。 */
export function formatTimeSmart(iso: string): string {
  const d = parse(iso)
  if (!d) return iso
  const p = parts(d, DATE_FIELDS)
  if (beijingDateKey(d) === beijingTodayKey()) return beijingHm(d)
  return `${p.month}-${p.day} ${beijingHm(d)}`
}

/** 当天显示 HH:mm，跨天显示 MM-DD（均北京时间）。 */
export function formatTimeShort(iso: string): string {
  const d = parse(iso)
  if (!d) return iso
  if (beijingDateKey(d) === beijingTodayKey()) return beijingHm(d)
  const p = parts(d, DATE_FIELDS)
  return `${p.month}-${p.day}`
}