/*
 * Dashboard quick-entry card for the runtime log viewer.
 */
import { useCallback, useState } from 'react'
import { Button, Card, CardContent, Typography, Chip, Box } from '@mui/material'
import { ArrowForward, Article } from '@mui/icons-material'
import { useNavigate } from 'react-router-dom'
import { api } from '@/api'
import { useAdaptivePolling } from '@/hooks/useAdaptivePolling'
import { useRefreshInterval } from '@/contexts/RefreshContext'

export function LogQuickEntry() {
  const navigate = useNavigate()
  const { refreshInterval, refreshKey } = useRefreshInterval()
  const [warnCount, setWarnCount] = useState(0)
  const [errorCount, setErrorCount] = useState(0)

  const load = useCallback(async () => {
    try {
      const response = await api.getLogs(1, 500)
      const entries = response.data?.entries ?? []
      setWarnCount(entries.filter((e) => e.level === 'warn').length)
      setErrorCount(entries.filter((e) => e.level === 'error').length)
    } catch {
      // 静默失败：日志条目非关键，不打断首页
    }
  }, [])

  useAdaptivePolling({
    refreshInterval: refreshInterval > 0 ? Math.max(refreshInterval * 4, 20_000) : 0,
    refreshKey,
    onTick: load,
    immediate: true,
    hiddenMinInterval: 120_000,
  })

  return (
    <Card sx={{ height: '100%' }}>
      <CardContent sx={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
        <Box display="flex" alignItems="center" gap={1} mb={1}>
          <Article color="primary" fontSize="small" />
          <Typography variant="subtitle2" color="text.secondary">
            系统日志
          </Typography>
        </Box>
        <Box display="flex" gap={1} mb={1.5}>
          <Chip size="small" label={`${warnCount} 警告`} color="warning" variant="outlined" />
          <Chip size="small" label={`${errorCount} 错误`} color="error" variant="outlined" />
        </Box>
        <Button
          size="small"
          endIcon={<ArrowForward />}
          onClick={() => void navigate('/logs')}
          sx={{ mt: 'auto', alignSelf: 'flex-start' }}
        >
          查看运行 / 报错日志
        </Button>
      </CardContent>
    </Card>
  )
}