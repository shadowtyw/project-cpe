/*
 * @Author: 1orz cloudorzi@gmail.com
 * @Date: 2025-12-10 10:16:54
 * @LastEditors: 1orz cloudorzi@gmail.com
 * @LastEditTime: 2025-12-13 12:44:28
 * @FilePath: /udx710-backend/frontend/src/pages/Dashboard/components/QuickControls.tsx
 * @Description:
 *
 * Copyright (c) 2025 by 1orz, All Rights Reserved.
 */
import { Box, Card, CardContent, Typography, Stack, Switch, Chip, CircularProgress } from '@mui/material'
import { NetworkCheck, FlightTakeoff, TravelExplore, CellTower } from '@mui/icons-material'
import type { AirplaneModeResponse, RoamingResponse, RadioMode } from '@/api/types'

interface QuickControlsProps {
  dataStatus: boolean | null
  airplaneMode: AirplaneModeResponse | null
  roaming: RoamingResponse | null
  radioMode: RadioMode | null
  radioModePending: boolean
  onToggleData: () => void
  onToggleAirplaneMode: () => void
  onToggleRoaming: () => void
  onToggle5g: () => void
}

export function QuickControls({
  dataStatus,
  airplaneMode,
  roaming,
  radioMode,
  radioModePending,
  onToggleData,
  onToggleAirplaneMode,
  onToggleRoaming,
  onToggle5g,
}: QuickControlsProps) {
  return (
    <Card sx={{ height: '100%' }}>
      <CardContent>
        <Typography variant="subtitle2" color="text.secondary" gutterBottom>
          快捷控制
        </Typography>
        <Stack spacing={2}>
          <Box display="flex" alignItems="center" justifyContent="space-between">
            <Box display="flex" alignItems="center" gap={1}>
              <NetworkCheck color={dataStatus ? 'success' : 'disabled'} />
              <Typography variant="body2">数据连接</Typography>
            </Box>
            <Switch
              checked={dataStatus ?? false}
              onChange={onToggleData}
              color="success"
              size="small"
              disabled={dataStatus === null}
            />
          </Box>

          <Box display="flex" alignItems="center" justifyContent="space-between">
            <Box display="flex" alignItems="center" gap={1}>
              {radioModePending ? (
                <CircularProgress size={16} />
              ) : (
                <CellTower
                  color={radioMode === null ? 'disabled' : radioMode === 'lte' ? 'info' : 'success'}
                />
              )}
              <Typography variant="body2">5G 移动网络</Typography>
            </Box>
            <Switch
              checked={radioMode !== null && radioMode !== 'lte'}
              onChange={onToggle5g}
              color="success"
              size="small"
              disabled={radioMode === null || radioModePending}
            />
          </Box>

          <Box display="flex" alignItems="center" justifyContent="space-between">
            <Box display="flex" alignItems="center" gap={1}>
              <TravelExplore color={roaming?.roaming_allowed ? 'info' : 'disabled'} />
              <Typography variant="body2">数据漫游</Typography>
              {roaming?.is_roaming && (
                <Chip label="漫游中" size="small" color="warning" sx={{ height: 18, fontSize: '0.65rem' }} />
              )}
            </Box>
            <Switch
              checked={roaming?.roaming_allowed ?? false}
              onChange={onToggleRoaming}
              color="info"
              size="small"
              disabled={!roaming}
            />
          </Box>

          <Box display="flex" alignItems="center" justifyContent="space-between">
            <Box display="flex" alignItems="center" gap={1}>
              <FlightTakeoff color={airplaneMode?.enabled ? 'warning' : 'disabled'} />
              <Typography variant="body2">飞行模式</Typography>
            </Box>
            <Switch
              checked={airplaneMode?.enabled ?? false}
              onChange={onToggleAirplaneMode}
              color="warning"
              size="small"
              disabled={!airplaneMode}
            />
          </Box>
        </Stack>
      </CardContent>
    </Card>
  )
}
