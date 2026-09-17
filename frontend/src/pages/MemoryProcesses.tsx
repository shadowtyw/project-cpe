/*
 * Top memory process page.
 */
import { useCallback, useState } from 'react'
import {
  Alert,
  Box,
  Button,
  Chip,
  CircularProgress,
  Paper,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  Tooltip,
  Typography,
} from '@mui/material'
import { Memory, Refresh } from '@mui/icons-material'
import { api } from '../api'
import { useRefreshInterval } from '../contexts/RefreshContext'
import { useAdaptivePolling } from '../hooks/useAdaptivePolling'
import type { MemoryProcessesResponse } from '../api/types'

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

export default function MemoryProcesses() {
  const { refreshInterval, refreshKey } = useRefreshInterval()
  const [data, setData] = useState<MemoryProcessesResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    setRefreshing(true)
    try {
      const response = await api.getMemoryProcesses()
      if (response.data) {
        setData(response.data)
        setError(null)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
      setRefreshing(false)
    }
  }, [])

  useAdaptivePolling({
    refreshInterval: refreshInterval > 0 ? Math.max(refreshInterval, 10_000) : 0,
    refreshKey,
    onTick: load,
    immediate: true,
    hiddenMinInterval: 60_000,
  })

  return (
    <Box>
      <Box display="flex" justifyContent="space-between" alignItems="center" mb={3}>
        <Box display="flex" alignItems="center" gap={1}>
          <Memory color="primary" />
          <Box>
            <Typography variant="h4" fontWeight={600}>内存占用进程</Typography>
            <Typography variant="body2" color="text.secondary">按 RSS 占用排序的前 10 个进程</Typography>
          </Box>
        </Box>
        <Button variant="outlined" startIcon={refreshing ? <CircularProgress size={18} /> : <Refresh />} onClick={() => void load()} disabled={refreshing}>刷新</Button>
      </Box>

      <Alert severity="info" sx={{ mb: 2 }}>
        RSS 是进程当前驻留内存的近似值，共享内存可能被多个进程重复计数；此页面只读，不提供终止进程或执行命令的功能。
      </Alert>
      {error && <Alert severity="error" sx={{ mb: 2 }}>{error}</Alert>}

      {loading && !data ? (
        <Box display="flex" justifyContent="center" py={8}><CircularProgress /></Box>
      ) : data ? (
        <>
          <Box display="flex" gap={1} mb={2}>
            <Chip label={`可读进程 ${data.total_processes}`} />
            <Chip label={`采样时间 ${new Date(data.sampled_at).toLocaleString()}`} variant="outlined" />
          </Box>
          <TableContainer component={Paper}>
            <Table size="small">
              <TableHead><TableRow>
                <TableCell>#</TableCell><TableCell>PID</TableCell><TableCell>进程</TableCell><TableCell>命令</TableCell>
                <TableCell align="right">RSS</TableCell><TableCell align="right">虚拟内存</TableCell><TableCell align="right">内存占比</TableCell><TableCell align="right">线程</TableCell>
              </TableRow></TableHead>
              <TableBody>
                {data.processes.map((process, index) => (
                  <TableRow key={`${process.pid}-${process.name}`} hover>
                    <TableCell>{index + 1}</TableCell>
                    <TableCell>{process.pid}</TableCell>
                    <TableCell sx={{ fontWeight: 600 }}>{process.name}</TableCell>
                    <TableCell sx={{ maxWidth: 420, fontFamily: 'monospace', fontSize: '0.75rem', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                      <Tooltip title={process.command}><span>{process.command}</span></Tooltip>
                    </TableCell>
                    <TableCell align="right">{formatBytes(process.rss_bytes)}</TableCell>
                    <TableCell align="right">{formatBytes(process.virtual_bytes)}</TableCell>
                    <TableCell align="right">{process.memory_percent.toFixed(2)}%</TableCell>
                    <TableCell align="right">{process.threads}</TableCell>
                  </TableRow>
                ))}
                {data.processes.length === 0 && <TableRow><TableCell colSpan={8} align="center">没有可读取的进程</TableCell></TableRow>}
              </TableBody>
            </Table>
          </TableContainer>
        </>
      ) : null}
    </Box>
  )
}
