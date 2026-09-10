/*
 * 高级工具页面：流量统计、定时计划、短信遥控、诊断与备份。
 *
 * 每个标签页都是独立的只读/写组件，复用统一 api 客户端与轮询机制。
 * 所有新功能默认关闭，不影响设备长期稳定运行。
 */
import { useCallback, useState } from 'react'
import {
  Alert,
  Box,
  Button,
  Card,
  CardContent,
  Chip,
  CircularProgress,
  FormControl,
  FormControlLabel,
  InputLabel,
  MenuItem,
  Paper,
  Select,
  Switch,
  Tab,
  Tabs,
  TextField,
  Typography,
} from '@mui/material'
import {
  Download,
  Upload,
  Schedule as ScheduleIcon,
  DataUsage,
  Storage,
} from '@mui/icons-material'
import { api } from '../api'
import { useRefreshInterval } from '../contexts/RefreshContext'
import { useAdaptivePolling } from '../hooks/useAdaptivePolling'
import type {
  TrafficStatsResponse,
  ScheduleConfig,
  ScheduleEntry,
  ScheduleAction,
  DiagnosticReport,
} from '../api/types'

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  const units = ['KiB', 'MiB', 'GiB', 'TiB']
  let value = bytes / 1024
  let index = 0
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024
    index += 1
  }
  return `${value.toFixed(value >= 100 ? 0 : value >= 10 ? 1 : 2)} ${units[index]}`
}

// ============ 流量统计 ============

function TrafficPanel() {
  const { refreshInterval, refreshKey } = useRefreshInterval()
  const [data, setData] = useState<TrafficStatsResponse | null>(null)
  const [enabled, setEnabled] = useState(false)
  const [threshold, setThreshold] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)

  const load = useCallback(async () => {
    try {
      const response = await api.getTrafficStats()
      if (response.data) {
        setData(response.data)
        setEnabled(response.data.alert_enabled)
        setThreshold(String(response.data.daily_threshold_bytes ?? ''))
        setError(null)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [])

  useAdaptivePolling({
    refreshInterval: refreshInterval > 0 ? Math.max(refreshInterval, 10_000) : 0,
    refreshKey,
    onTick: load,
    immediate: true,
    hiddenMinInterval: 60_000,
  })

  const saveAlert = async () => {
    setSaving(true)
    try {
      const bytes = Number(threshold) || 0
      await api.setTrafficAlert({ enabled, daily_threshold_bytes: bytes })
      setError(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Box>
      <Box display="flex" gap={2} mb={3} flexWrap="wrap">
        <Card sx={{ flex: 1, minWidth: 180 }}>
          <CardContent>
            <Typography variant="caption" color="text.secondary">今日下行</Typography>
            <Typography variant="h5" fontWeight={600}>{formatBytes(data?.today_rx_bytes ?? 0)}</Typography>
          </CardContent>
        </Card>
        <Card sx={{ flex: 1, minWidth: 180 }}>
          <CardContent>
            <Typography variant="caption" color="text.secondary">今日上行</Typography>
            <Typography variant="h5" fontWeight={600}>{formatBytes(data?.today_tx_bytes ?? 0)}</Typography>
          </CardContent>
        </Card>
        <Card sx={{ flex: 1, minWidth: 180 }}>
          <CardContent>
            <Typography variant="caption" color="text.secondary">本月下行</Typography>
            <Typography variant="h5" fontWeight={600}>{formatBytes(data?.month_rx_bytes ?? 0)}</Typography>
          </CardContent>
        </Card>
        <Card sx={{ flex: 1, minWidth: 180 }}>
          <CardContent>
            <Typography variant="caption" color="text.secondary">本月上行</Typography>
            <Typography variant="h5" fontWeight={600}>{formatBytes(data?.month_tx_bytes ?? 0)}</Typography>
          </CardContent>
        </Card>
      </Box>

      <Paper variant="outlined" sx={{ p: 2, mb: 2 }}>
        <Typography variant="subtitle2" gutterBottom>用量预警</Typography>
        <FormControlLabel
          control={<Switch checked={enabled} onChange={(e) => setEnabled(e.target.checked)} />}
          label="启用单日流量阈值预警（触发后记录到系统日志）"
        />
        <Box display="flex" gap={1} alignItems="center" mt={1} flexWrap="wrap">
          <TextField
            label="单日阈值（字节）"
            value={threshold}
            onChange={(e) => setThreshold(e.target.value)}
            size="small"
            type="number"
            sx={{ minWidth: 200 }}
          />
          <Button variant="contained" size="small" onClick={() => void saveAlert()} disabled={saving}>
            {saving ? <CircularProgress size={18} /> : '保存'}
          </Button>
        </Box>
      </Paper>

      <Typography variant="body2" color="text.secondary">
        流量通过读取网络接口计数器累加，每 60 秒写入一次数据库，对设备 flash 影响极小。
      </Typography>
      {error && <Alert severity="error" sx={{ mt: 2 }}>{error}</Alert>}
    </Box>
  )
}

// ============ 定时计划 ============

const SCHEDULE_ACTIONS: { value: ScheduleAction; label: string }[] = [
  { value: 'airplane_on', label: '开启飞行模式' },
  { value: 'airplane_off', label: '关闭飞行模式' },
  { value: 'data_on', label: '开启数据连接' },
  { value: 'data_off', label: '关闭数据连接' },
  { value: 'radio_lte', label: '仅 4G LTE' },
  { value: 'radio_nr', label: '仅 5G NR' },
  { value: 'radio_auto', label: '4G/5G 自动' },
  { value: 'radio_off', label: '关闭射频' },
  { value: 'reboot', label: '重启设备' },
]

function SchedulePanel() {
  const { refreshInterval, refreshKey } = useRefreshInterval()
  const [config, setConfig] = useState<ScheduleConfig | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)

  const load = useCallback(async () => {
    try {
      const response = await api.getScheduleConfig()
      if (response.data) {
        setConfig(response.data)
        setError(null)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [])

  useAdaptivePolling({
    refreshInterval: refreshInterval > 0 ? Math.max(refreshInterval, 15_000) : 0,
    refreshKey,
    onTick: load,
    immediate: true,
    hiddenMinInterval: 60_000,
  })

  const save = async () => {
    if (!config) return
    setSaving(true)
    try {
      await api.setScheduleConfig(config)
      setError(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  const addEntry = () => {
    if (!config) return
    const entry: ScheduleEntry = { enabled: false, time: '00:00', weekdays: [], action: 'reboot' }
    setConfig({ ...config, entries: [...config.entries, entry] })
  }

  const updateEntry = (index: number, patch: Partial<ScheduleEntry>) => {
    if (!config) return
    const entries = config.entries.map((e, i) => (i === index ? { ...e, ...patch } : e))
    setConfig({ ...config, entries })
  }

  const removeEntry = (index: number) => {
    if (!config) return
    setConfig({ ...config, entries: config.entries.filter((_, i) => i !== index) })
  }

  return (
    <Box>
      <Typography variant="body2" color="text.secondary" mb={2}>
        在指定时间自动执行省电或管理动作。每次 30 秒检查一次，容差窗口内触发。
      </Typography>
      {config?.entries.map((entry, index) => (
        <Paper key={index} variant="outlined" sx={{ p: 2, mb: 1.5 }}>
          <Box display="flex" gap={1.5} alignItems="center" flexWrap="wrap">
            <FormControlLabel
              control={<Switch checked={entry.enabled} onChange={(e) => updateEntry(index, { enabled: e.target.checked })} />}
              label={entry.enabled ? '启用' : '关闭'}
            />
            <TextField
              label="时间 HH:MM"
              size="small"
              value={entry.time}
              onChange={(e) => updateEntry(index, { time: e.target.value })}
              sx={{ width: 120 }}
            />
            <FormControl size="small" sx={{ minWidth: 180 }}>
              <InputLabel id={`schedule-action-label-${index}`}>动作</InputLabel>
              <Select
                labelId={`schedule-action-label-${index}`}
                label="动作"
                value={entry.action}
                onChange={(e) => updateEntry(index, { action: e.target.value as ScheduleAction })}
              >
                {SCHEDULE_ACTIONS.map((a) => (
                  <MenuItem key={a.value} value={a.value}>{a.label}</MenuItem>
                ))}
              </Select>
            </FormControl>
            <Button size="small" color="error" onClick={() => removeEntry(index)}>删除</Button>
          </Box>
        </Paper>
      ))}
      <Button variant="outlined" size="small" onClick={addEntry} sx={{ mr: 1 }}>添加计划</Button>
      <Button variant="contained" size="small" onClick={() => void save()} disabled={saving || !config}>
        {saving ? <CircularProgress size={18} /> : '保存'}
      </Button>
      {error && <Alert severity="error" sx={{ mt: 2 }}>{error}</Alert>}
    </Box>
  )
}

// ============ 诊断与备份 ============

function DiagnosticsPanel() {
  const [report, setReport] = useState<DiagnosticReport | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  const generate = async () => {
    setLoading(true)
    setError(null)
    try {
      const response = await api.getDiagnosticReport()
      if (response.data) setReport(response.data)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }

  const downloadReport = () => {
    if (!report) return
    const blob = new Blob([JSON.stringify(report, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `udx710-diagnostics-${report.generated_at.replace(/[:.]/g, '-')}.json`
    a.click()
    URL.revokeObjectURL(url)
  }

  const exportConfig = async () => {
    setError(null)
    try {
      const response = await api.exportConfig()
      const blob = new Blob([JSON.stringify(response.data, null, 2)], { type: 'application/json' })
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = 'config.json'
      a.click()
      URL.revokeObjectURL(url)
      setNotice('配置已导出')
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const importConfig = async (file: File) => {
    setError(null)
    setNotice(null)
    try {
      const text = await file.text()
      const config = JSON.parse(text) as Record<string, unknown>
      await api.importConfig(config)
      setNotice('配置已导入')
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  return (
    <Box>
      <Box display="flex" gap={1.5} mb={2} flexWrap="wrap">
        <Button variant="contained" startIcon={loading ? <CircularProgress size={18} /> : <Storage />} onClick={() => void generate()} disabled={loading}>
          生成诊断报告
        </Button>
        {report && (
          <Button variant="outlined" startIcon={<Download />} onClick={downloadReport}>
            下载报告
          </Button>
        )}
        <Button variant="outlined" startIcon={<Download />} onClick={() => void exportConfig()}>
          导出配置
        </Button>
        <Button variant="outlined" component="label" startIcon={<Upload />}>
          导入配置
          <input type="file" hidden accept="application/json" onChange={(e) => { const f = e.target.files?.[0]; if (f) void importConfig(f) }} />
        </Button>
      </Box>

      {notice && <Alert severity="success" sx={{ mb: 2 }}>{notice}</Alert>}
      {error && <Alert severity="error" sx={{ mb: 2 }}>{error}</Alert>}

      {report && (
        <Paper variant="outlined" sx={{ p: 2 }}>
          <Typography variant="subtitle2" gutterBottom>诊断概览</Typography>
          <Box display="flex" gap={1} flexWrap="wrap">
            <Chip label={`版本 ${report.version}`} size="small" />
            <Chip label={`commit ${report.commit}`} size="small" variant="outlined" />
            <Chip label={`设备 ${report.device?.model || '未知'}`} size="small" variant="outlined" />
            <Chip label={`运营商 ${report.network?.operator_name || '未知'}`} size="small" variant="outlined" />
            <Chip label={`创建 ${new Date(report.generated_at).toLocaleString()}`} size="small" variant="outlined" />
          </Box>
          {report.recent_logs.length > 0 && (
            <Box mt={2}>
              <Typography variant="caption" color="text.secondary">最近日志（{report.recent_logs.length} 条）</Typography>
              <Box
                sx={{
                  mt: 1, p: 1.5, backgroundColor: '#1e1e1e', color: '#d4d4d4',
                  fontFamily: 'monospace', fontSize: '0.75rem', maxHeight: 240, overflow: 'auto', borderRadius: 1,
                }}
              >
                {report.recent_logs.map((log, i) => (
                  <div key={i}>{log.timestamp} [{log.level}] {log.module}: {log.message}</div>
                ))}
              </Box>
            </Box>
          )}
        </Paper>
      )}
    </Box>
  )
}

// ============ 页面入口 ============

export default function Tools() {
  const [tab, setTab] = useState(0)

  return (
    <Box>
      <Box display="flex" alignItems="center" gap={1} mb={2}>
        <Typography variant="h4" fontWeight={600}>高级工具</Typography>
      </Box>
      <Tabs value={tab} onChange={(_, value: number) => setTab(value)} sx={{ mb: 2, borderBottom: 1, borderColor: 'divider' }}>
        <Tab icon={<DataUsage />} iconPosition="start" label="流量统计" />
        <Tab icon={<ScheduleIcon />} iconPosition="start" label="定时计划" />
        <Tab icon={<Storage />} iconPosition="start" label="诊断与备份" />
      </Tabs>

      <Box mt={2}>
        {tab === 0 && <TrafficPanel />}
        {tab === 1 && <SchedulePanel />}
        {tab === 2 && <DiagnosticsPanel />}
      </Box>
    </Box>
  )
}