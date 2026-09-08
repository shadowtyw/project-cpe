/*
 * Runtime log viewer page.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import {
  Alert,
  Box,
  Button,
  Chip,
  CircularProgress,
  FormControl,
  InputLabel,
  MenuItem,
  Paper,
  Select,
  Typography,
} from '@mui/material'
import { Refresh, DeleteSweep, Terminal } from '@mui/icons-material'
import { api } from '../api'
import { useRefreshInterval } from '../contexts/RefreshContext'
import { useAdaptivePolling } from '../hooks/useAdaptivePolling'
import type { LogEntry } from '../api/types'

// 日志等级筛选：min_level 数值（0=debug 1=info 2=warn 3=error）
const LEVEL_OPTIONS = [
  { value: 0, label: '全部日志' },
  { value: 1, label: '运行日志 (info+)' },
  { value: 2, label: '报错日志 (warn+)' },
] as const

const LEVEL_COLORS: Record<LogEntry['level'], string> = {
  debug: '#9e9e9e',
  info: '#4fc3f7',
  warn: '#ffb74d',
  error: '#ef5350',
}

function formatTime(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleTimeString('zh-CN', { hour12: false })
}

export default function Logs() {
  const { refreshInterval, refreshKey } = useRefreshInterval()
  const [entries, setEntries] = useState<LogEntry[]>([])
  const [loading, setLoading] = useState(true)
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [minLevel, setMinLevel] = useState<number>(1)
  const scrollRef = useRef<HTMLDivElement>(null)

  const load = useCallback(async (level: number, showSpinner = false) => {
    if (showSpinner) setRefreshing(true)
    try {
      const response = await api.getLogs(level, 500)
      if (response.data) {
        setEntries(response.data.entries)
        setError(null)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
      if (showSpinner) setRefreshing(false)
    }
  }, [])

  const handleLevelChange = (value: number) => {
    setMinLevel(value)
    void load(value, true)
  }

  const handleManualRefresh = () => {
    void load(minLevel, true)
  }

  const handleClear = async () => {
    try {
      await api.clearLogs()
      setEntries([])
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  useAdaptivePolling({
    refreshInterval: refreshInterval > 0 ? Math.max(refreshInterval, 5_000) : 0,
    refreshKey,
    onTick: () => load(minLevel),
    immediate: true,
    hiddenMinInterval: 60_000,
  })

  // 新增日志时保持底部可见（日志最新在顶部，此处不强制滚动）
  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = 0
    }
  }, [entries.length])

  return (
    <Box>
      <Box display="flex" justifyContent="space-between" alignItems="center" mb={3} flexWrap="wrap" gap={2}>
        <Box display="flex" alignItems="center" gap={1}>
          <Terminal color="primary" />
          <Box>
            <Typography variant="h4" fontWeight={600}>系统日志</Typography>
            <Typography variant="body2" color="text.secondary">进程内存环形缓冲，重启后清空</Typography>
          </Box>
        </Box>
        <Box display="flex" gap={1} alignItems="center">
          <FormControl size="small" sx={{ minWidth: 180 }}>
            <InputLabel id="log-level-label">日志级别</InputLabel>
            <Select
              labelId="log-level-label"
              value={minLevel}
              label="日志级别"
              onChange={(e) => handleLevelChange(Number(e.target.value))}
            >
              {LEVEL_OPTIONS.map((option) => (
                <MenuItem key={option.value} value={option.value}>
                  {option.label}
                </MenuItem>
              ))}
            </Select>
          </FormControl>
          <Button
            variant="outlined"
            startIcon={refreshing ? <CircularProgress size={18} /> : <Refresh />}
            onClick={handleManualRefresh}
            disabled={refreshing}
          >
            刷新
          </Button>
          <Button
            variant="outlined"
            color="error"
            startIcon={<DeleteSweep />}
            onClick={() => void handleClear()}
          >
            清空
          </Button>
        </Box>
      </Box>

      {error && <Alert severity="error" sx={{ mb: 2 }}>{error}</Alert>}

      <Box display="flex" gap={1} mb={2}>
        <Chip label={`${entries.length} 条`} size="small" variant="outlined" />
        <Typography variant="caption" color="text.secondary" sx={{ alignSelf: 'center' }}>
          提示：日志仅保存在内存中，不会写入设备闪存
        </Typography>
      </Box>

      {loading ? (
        <Box display="flex" justifyContent="center" py={8}><CircularProgress /></Box>
      ) : entries.length === 0 ? (
        <Alert severity="info">当前级别下没有日志记录</Alert>
      ) : (
        <Paper
          variant="outlined"
          ref={scrollRef}
          sx={{
            backgroundColor: '#1e1e1e',
            color: '#d4d4d4',
            fontFamily: 'monospace',
            fontSize: '0.8rem',
            p: 1.5,
            maxHeight: '68vh',
            overflow: 'auto',
          }}
        >
          {entries.map((entry, index) => (
            <Box key={index} sx={{ py: 0.25, display: 'flex', gap: 1, lineHeight: 1.6 }}>
              <Typography component="span" sx={{ color: '#6a9955', whiteSpace: 'nowrap' }}>
                {formatTime(entry.timestamp)}
              </Typography>
              <Typography component="span" sx={{ color: LEVEL_COLORS[entry.level], width: '3.5ch', whiteSpace: 'nowrap' }}>
                {entry.level.toUpperCase()}
              </Typography>
              <Typography component="span" sx={{ color: '#808080', whiteSpace: 'nowrap' }}>
                {entry.module}
              </Typography>
              <Typography component="span" sx={{ wordBreak: 'break-all' }}>
                {entry.message}
              </Typography>
            </Box>
          ))}
        </Paper>
      )}
    </Box>
  )
}