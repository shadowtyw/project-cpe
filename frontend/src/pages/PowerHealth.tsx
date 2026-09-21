/*
 * 低功耗健康看板（一期 — 只读）。
 *
 * 直观展示 v3.8.0–3.8.5 低功耗重构的运行效果：当前 CPU 空闲度、中断唤醒率、
 * 以及各后台看门狗/采样循环的规划间隔对照表。本页面不修改任何配置或后台任务。
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
  Grid,
  Paper,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  Typography,
} from '@mui/material'
import { PowerSettingsNew as PowerIcon, Refresh } from '@mui/icons-material'
import { LineChart } from '@mui/x-charts/LineChart'
import { api } from '../api'
import { useRefreshInterval } from '../contexts/RefreshContext'
import { useAdaptivePolling } from '../hooks/useAdaptivePolling'
import type {
  PlannedInterval,
  PowerHealthResponse,
} from '../api/types'
import { formatTimeHms } from '../utils/time'

/** 空闲度趋势环形缓冲上限 */
const IDLE_HISTORY_MAX = 30

interface IdlePoint {
  t: string // ISO 时间戳
  idle: number
}

const CLASSIFICATION_COLOR: Record<string, 'success' | 'warning' | 'info' | 'default' | 'secondary'> = {
  low_power: 'success',
  active: 'warning',
  adaptive: 'info',
  event_driven: 'default',
  on_demand: 'secondary',
}

const CLASSIFICATION_LABEL: Record<string, string> = {
  low_power: '低频省电',
  active: '高频活跃',
  adaptive: '自适应',
  event_driven: '事件驱动',
  on_demand: '按需',
}

function renderWakeups(data: PowerHealthResponse): string {
  const w = data.wakeups
  if (!w || w.window_secs <= 0) return '采集基线中…'
  if (w.irqs_per_sec >= 1000) return `${(w.irqs_per_sec / 1000).toFixed(1)}k/s`
  return `${w.irqs_per_sec.toFixed(0)}/s`
}

export default function PowerHealth() {
  const { refreshInterval, refreshKey } = useRefreshInterval()
  const [data, setData] = useState<PowerHealthResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [idleHistory, setIdleHistory] = useState<IdlePoint[]>([])

  const load = useCallback(async () => {
    setRefreshing(true)
    try {
      const response = await api.getPowerHealth()
      if (response.data) {
        setData(response.data)
        const idle = response.data.cpu?.idle_percent
        if (typeof idle === 'number' && !Number.isNaN(idle)) {
          const point: IdlePoint = { t: new Date().toISOString(), idle }
          setIdleHistory((prev) => [...prev.slice(-(IDLE_HISTORY_MAX - 1)), point])
        }
        setError(null)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
      setRefreshing(false)
    }
  }, [])

  const { effectiveRefreshInterval } = useAdaptivePolling({
    refreshInterval: refreshInterval > 0 ? Math.max(refreshInterval, 10_000) : 0,
    refreshKey,
    onTick: load,
    immediate: true,
    hiddenMinInterval: 60_000,
  })

  const cpu = data?.cpu
  const wakeups = data?.wakeups
  const intervals: PlannedInterval[] = data?.planned_intervals ?? []

  return (
    <Box>
      <Box display="flex" justifyContent="space-between" alignItems="center" mb={3}>
        <Box display="flex" alignItems="center" gap={1}>
          <PowerIcon color="primary" />
          <Box>
            <Typography variant="h4" fontWeight={600}>休眠状态</Typography>
            <Typography variant="body2" color="text.secondary">
              观察低功耗重构（v3.8.0–3.8.5）的运行效果：CPU 空闲度、中断唤醒率、后台任务规划周期
            </Typography>
          </Box>
        </Box>
        <Button
          variant="outlined"
          startIcon={refreshing ? <CircularProgress size={18} /> : <Refresh />}
          onClick={() => void load()}
          disabled={refreshing}
        >
          刷新
        </Button>
      </Box>

      <Alert severity="info" sx={{ mb: 2 }}>
        本页面只读，不修改任何看门狗或采样配置，仅观测诊断数据。中断唤醒率首次访问仅建立基线，需等待下一次轮询后才有数值。
      </Alert>
      {error && <Alert severity="error" sx={{ mb: 2 }}>{error}</Alert>}

      {loading && !data ? (
        <Box display="flex" justifyContent="center" py={8}><CircularProgress /></Box>
      ) : data ? (
        <Stack spacing={3}>
          {/* KPI 卡片 */}
          <Grid container spacing={2}>
            <Grid size={{ xs: 12, sm: 6, md: 3 }}>
              <Card>
                <CardContent>
                  <Typography variant="caption" color="text.secondary">CPU 空闲度</Typography>
                  <Typography variant="h5" fontWeight={600}>
                    {cpu?.idle_percent.toFixed(1)}%
                  </Typography>
                  <Typography variant="body2" color="text.secondary">
                    {'忙碌 '}
                    {cpu?.busy_percent.toFixed(1)}
                    {'% · '}
                    {formatTimeHms(data.sampled_at)}
                  </Typography>
                </CardContent>
              </Card>
            </Grid>
            <Grid size={{ xs: 12, sm: 6, md: 3 }}>
              <Card>
                <CardContent>
                  <Typography variant="caption" color="text.secondary">1 分钟负载</Typography>
                  <Typography variant="h5" fontWeight={600}>
                    {cpu?.load_1min.toFixed(2)}
                  </Typography>
                  <Typography variant="body2" color="text.secondary">
                    {`/ ${cpu?.core_count ?? 0} 核`}
                  </Typography>
                </CardContent>
              </Card>
            </Grid>
            <Grid size={{ xs: 12, sm: 6, md: 3 }}>
              <Card>
                <CardContent>
                  <Typography variant="caption" color="text.secondary">中断唤醒率</Typography>
                  <Typography variant="h5" fontWeight={600}>
                    {renderWakeups(data)}
                  </Typography>
                  <Typography variant="body2" color="text.secondary">
                    {wakeups
                      ? `累计 ${wakeups.total_interrupts.toLocaleString()} 次`
                      : '等待下一次轮询建立基线'}
                  </Typography>
                </CardContent>
              </Card>
            </Grid>
            <Grid size={{ xs: 12, sm: 6, md: 3 }}>
              <Card>
                <CardContent>
                  <Typography variant="caption" color="text.secondary">前端轮询间隔</Typography>
                  <Typography variant="h5" fontWeight={600}>
                    {`${(effectiveRefreshInterval / 1000).toFixed(0)}s`}
                  </Typography>
                  <Typography variant="body2" color="text.secondary">隐藏标签页时 ≥60s</Typography>
                </CardContent>
              </Card>
            </Grid>
          </Grid>

          {/* 空闲度趋势 */}
          <Card>
            <CardContent>
              <Typography variant="subtitle1" fontWeight="bold" gutterBottom>空闲度趋势</Typography>
              {idleHistory.length >= 2 ? (
                <LineChart
                  height={300}
                  series={[
                    {
                      data: idleHistory.map((h) => h.idle),
                      area: true,
                      showMark: false,
                      color: '#4caf50',
                      valueFormatter: (v) => (v === null || v === undefined ? '' : `${v.toFixed(1)}%`),
                    },
                  ]}
                  xAxis={[
                    {
                      data: idleHistory.map((h) => formatTimeHms(h.t)),
                      scaleType: 'point',
                    },
                  ]}
                  yAxis={[{ min: 0, max: 100 }]}
                  margin={{ left: 40, right: 16, top: 16, bottom: 36 }}
                />
              ) : (
                <Box display="flex" justifyContent="center" alignItems="center" height={300}>
                  <Typography color="text.secondary">采集基线中…（需至少 2 次采样）</Typography>
                </Box>
              )}
            </CardContent>
          </Card>

          {/* 看门狗规划间隔表 */}
          <TableContainer component={Paper}>
            <Table size="small">
              <TableHead>
                <TableRow>
                  <TableCell>任务</TableCell>
                  <TableCell>当前间隔</TableCell>
                  <TableCell>模式</TableCell>
                  <TableCell>状态</TableCell>
                  <TableCell>说明</TableCell>
                </TableRow>
              </TableHead>
              <TableBody>
                {intervals.map((row) => (
                  <TableRow key={row.key} hover>
                    <TableCell sx={{ fontWeight: 600 }}>{row.name}</TableCell>
                    <TableCell sx={{ whiteSpace: 'nowrap' }}>{row.interval_text}</TableCell>
                    <TableCell>
                      <Chip
                        size="small"
                        label={CLASSIFICATION_LABEL[row.classification] ?? row.classification}
                        color={CLASSIFICATION_COLOR[row.classification] ?? 'default'}
                        variant="outlined"
                      />
                    </TableCell>
                    <TableCell>
                      <Chip size="small" label={row.enabled ? '已启用' : '已禁用'} color={row.enabled ? 'success' : 'default'} variant="outlined" />
                    </TableCell>
                    <TableCell sx={{ color: 'text.secondary' }}>{row.note}</TableCell>
                  </TableRow>
                ))}
                {intervals.length === 0 && (
                  <TableRow><TableCell colSpan={5} align="center">暂无规划间隔数据</TableCell></TableRow>
                )}
              </TableBody>
            </Table>
          </TableContainer>
        </Stack>
      ) : null}
    </Box>
  )
}