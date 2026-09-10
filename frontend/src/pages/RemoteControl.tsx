import { useState, useEffect, useCallback } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import {
  Box,
  Typography,
  Tabs,
  Tab,
  Switch,
  FormControlLabel,
  Button,
  TextField,
  Alert,
  Chip,
  IconButton,
  Paper,
  Stack,
  CircularProgress,
  MenuItem as MuiMenuItem,
  Select,
  FormControl,
  Snackbar,
  Card,
  CardContent,
  Divider,
} from '@mui/material'
import {
  Add as AddIcon,
  Delete as DeleteIcon,
  Save as SaveIcon,
  Sms as SmsIcon,
  Phone as PhoneIcon,
  Refresh as RefreshIcon,
} from '@mui/icons-material'
import { api } from '../api'
import type {
  SmsControlConfigResponse,
  CallControlConfig,
  CallControlTrigger,
  ScheduleAction,
} from '../api/types'

const ACTION_LABELS: Record<ScheduleAction, string> = {
  reboot: '重启系统',
  airplane_on: '开启飞行模式',
  airplane_off: '关闭飞行模式',
  data_on: '开启数据连接',
  data_off: '关闭数据连接',
  radio_lte: '仅 4G',
  radio_nr: '仅 5G',
  radio_auto: '4G/5G 自动',
  radio_off: '关闭射频',
}

export default function RemoteControl() {
  const location = useLocation()
  const navigate = useNavigate()

  const activeTab = location.pathname.endsWith('/call') ? 1 : 0

  const handleTabChange = (_: unknown, newValue: number) => {
    void navigate(newValue === 0 ? '/remote/sms' : '/remote/call')
  }

  // SMS Control state
  const [smsConfig, setSmsConfig] = useState<SmsControlConfigResponse>({
    enabled: false,
    numbers: [],
  })
  const [smsLoading, setSmsLoading] = useState(false)
  const [smsInitialized, setSmsInitialized] = useState(false)

  // Call Control state
  const [callConfig, setCallConfig] = useState<CallControlConfig>({
    enabled: false,
    numbers: [],
    actions: [],
  })
  const [callTrigger, setCallTrigger] = useState<CallControlTrigger | null>(null)
  const [callLoading, setCallLoading] = useState(false)
  const [callInitialized, setCallInitialized] = useState(false)
  const [newNumber, setNewNumber] = useState('')

  // Snackbar state
  const [snackbar, setSnackbar] = useState<{
    open: boolean
    message: string
    severity: 'success' | 'error'
  }>({ open: false, message: '', severity: 'success' })

  const showSnackbar = (message: string, severity: 'success' | 'error') => {
    setSnackbar({ open: true, message, severity })
  }

  // Load SMS config
  const loadSmsConfig = useCallback(async () => {
    try {
      const res = await api.getSmsControlConfig()
      if (res.data) {
        setSmsConfig(res.data)
      }
    } catch {
      showSnackbar('加载短信遥控配置失败', 'error')
    } finally {
      setSmsInitialized(true)
    }
  }, [])

  // Load Call config
  const loadCallConfig = useCallback(async () => {
    try {
      const [configRes, statusRes] = await Promise.all([
        api.getCallControlConfig(),
        api.getCallControlStatus(),
      ])
      if (configRes.data) {
        setCallConfig(configRes.data)
      }
      if (statusRes.data) {
        setCallTrigger(statusRes.data)
      }
    } catch {
      showSnackbar('加载通话遥控配置失败', 'error')
    } finally {
      setCallInitialized(true)
    }
  }, [])

  useEffect(() => {
    void loadSmsConfig()
    void loadCallConfig()
  }, [loadSmsConfig, loadCallConfig])

  // Save SMS config
  const handleSaveSms = async () => {
    setSmsLoading(true)
    try {
      const res = await api.setSmsControlConfig(smsConfig.enabled)
      if (res.data) {
        setSmsConfig(res.data)
        showSnackbar('短信遥控配置已保存', 'success')
      }
    } catch {
      showSnackbar('保存短信遥控配置失败', 'error')
    } finally {
      setSmsLoading(false)
    }
  }

  // Save Call config
  const handleSaveCall = async () => {
    setCallLoading(true)
    try {
      const res = await api.setCallControlConfig(callConfig)
      if (res.data) {
        setCallConfig(res.data)
        showSnackbar('通话遥控配置已保存', 'success')
      }
    } catch {
      showSnackbar('保存通话遥控配置失败', 'error')
    } finally {
      setCallLoading(false)
    }
  }

  // Whitelist management
  const handleAddNumber = () => {
    const trimmed = newNumber.trim()
    if (trimmed && !callConfig.numbers.includes(trimmed)) {
      setCallConfig({
        ...callConfig,
        numbers: [...callConfig.numbers, trimmed],
      })
      setNewNumber('')
    }
  }

  const handleDeleteNumber = (num: string) => {
    setCallConfig({
      ...callConfig,
      numbers: callConfig.numbers.filter((n) => n !== num),
    })
  }

  // Refresh trigger status
  const handleRefreshStatus = async () => {
    try {
      const res = await api.getCallControlStatus()
      if (res.data) {
        setCallTrigger(res.data)
      }
    } catch {
      showSnackbar('刷新状态失败', 'error')
    }
  }

  return (
    <Box sx={{ p: 3 }}>
      <Typography variant="h4" gutterBottom>
        远程遥控
      </Typography>

      <Tabs value={activeTab} onChange={handleTabChange} sx={{ mb: 3 }}>
        <Tab icon={<SmsIcon />} iconPosition="start" label="短信遥控" />
        <Tab icon={<PhoneIcon />} iconPosition="start" label="通话遥控" />
      </Tabs>

      {/* SMS Control Panel */}
      {activeTab === 0 && (
        <Paper sx={{ p: 3 }}>
          {!smsInitialized ? (
            <Box sx={{ display: 'flex', justifyContent: 'center', p: 4 }}>
              <CircularProgress />
            </Box>
          ) : (
            <Stack spacing={3}>
              <Alert severity="info">
                白名单号码发送指定格式的短信即可远程控制设备，无需网络连接。
              </Alert>

              <FormControlLabel
                control={
                  <Switch
                    checked={smsConfig.enabled}
                    onChange={(e) =>
                      setSmsConfig({ ...smsConfig, enabled: e.target.checked })
                    }
                  />
                }
                label="启用短信遥控"
              />

              <Alert severity="info">
                <Typography variant="subtitle2" gutterBottom>
                  支持的指令（中英文均可）：
                </Typography>
                <Box sx={{ display: 'flex', flexWrap: 'wrap', gap: 0.5, mt: 1 }}>
                  <Chip label="#状态#" size="small" color="primary" variant="outlined" />
                  <Chip label="#重启#" size="small" color="primary" variant="outlined" />
                  <Chip label="#重连#" size="small" color="primary" variant="outlined" />
                  <Chip label="#飞行开#" size="small" color="primary" variant="outlined" />
                  <Chip label="#飞行关#" size="small" color="primary" variant="outlined" />
                  <Chip label="#数据开#" size="small" color="primary" variant="outlined" />
                  <Chip label="#数据关#" size="small" color="primary" variant="outlined" />
                  <Chip label="#仅4G#" size="small" color="primary" variant="outlined" />
                  <Chip label="#仅5G#" size="small" color="primary" variant="outlined" />
                  <Chip label="#自动#" size="small" color="primary" variant="outlined" />
                  <Chip label="#关射频#" size="small" color="primary" variant="outlined" />
                </Box>
                <Typography variant="caption" color="text.secondary" sx={{ mt: 1, display: 'block' }}>
                  也支持英文指令：#STATUS# #REBOOT# #RECONNECT# 等
                </Typography>
              </Alert>

              <Box>
                <Typography variant="subtitle2" gutterBottom>
                  白名单号码（在「通话遥控」中管理）
                </Typography>
                {smsConfig.numbers.length > 0 ? (
                  <Box sx={{ display: 'flex', flexWrap: 'wrap', gap: 1 }}>
                    {smsConfig.numbers.map((num) => (
                      <Chip key={num} label={num} />
                    ))}
                  </Box>
                ) : (
                  <Alert severity="warning">
                    尚未配置白名单号码，请在「通话遥控」中添加
                  </Alert>
                )}
              </Box>

              <Button
                variant="contained"
                startIcon={smsLoading ? <CircularProgress size={20} /> : <SaveIcon />}
                onClick={() => void handleSaveSms()}
                disabled={smsLoading}
              >
                {smsLoading ? '保存中...' : '保存配置'}
              </Button>
            </Stack>
          )}
        </Paper>
      )}

      {/* Call Control Panel */}
      {activeTab === 1 && (
        <Paper sx={{ p: 3 }}>
          {!callInitialized ? (
            <Box sx={{ display: 'flex', justifyContent: 'center', p: 4 }}>
              <CircularProgress />
            </Box>
          ) : (
            <Stack spacing={3}>
              <Alert severity="info">
                白名单号码拨打电话时，设备会自动接听，接通后所有动作同时计时，各自在到达等待时长后触发。
              </Alert>

              <FormControlLabel
                control={
                  <Switch
                    checked={callConfig.enabled}
                    onChange={(e) =>
                      setCallConfig({ ...callConfig, enabled: e.target.checked })
                    }
                  />
                }
                label="启用通话遥控"
              />

              {/* Trigger Status */}
              <Card variant="outlined">
                <CardContent>
                  <Box sx={{ display: 'flex', justifyContent: 'space-between', mb: 2 }}>
                    <Typography variant="subtitle1" fontWeight="bold">
                      上次触发状态
                    </Typography>
                    <IconButton size="small" onClick={() => void handleRefreshStatus()}>
                      <RefreshIcon />
                    </IconButton>
                  </Box>
                  {callTrigger && callTrigger.triggered_at ? (
                    <Stack spacing={1}>
                      <Typography variant="body2">
                        时间: {callTrigger.triggered_at}
                      </Typography>
                      <Typography variant="body2">
                        来源号码: {callTrigger.from_number || '未知'}
                      </Typography>
                      <Typography variant="body2">
                        执行动作: {callTrigger.action || '无'}
                      </Typography>
                    </Stack>
                  ) : (
                    <Typography variant="body2" color="text.secondary">
                      暂无触发记录
                    </Typography>
                  )}
                </CardContent>
              </Card>

              <Divider />

              {/* Whitelist */}
              <Box>
                <Typography variant="subtitle2" gutterBottom>
                  白名单号码
                </Typography>
                <Box sx={{ display: 'flex', gap: 1, mb: 2 }}>
                  <TextField
                    size="small"
                    placeholder="输入手机号码"
                    value={newNumber}
                    onChange={(e) => setNewNumber(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') {
                        e.preventDefault()
                        handleAddNumber()
                      }
                    }}
                    sx={{ flex: 1 }}
                  />
                  <IconButton color="primary" onClick={handleAddNumber}>
                    <AddIcon />
                  </IconButton>
                </Box>
                {callConfig.numbers.length > 0 ? (
                  <Box sx={{ display: 'flex', flexWrap: 'wrap', gap: 1 }}>
                    {callConfig.numbers.map((num) => (
                      <Chip
                        key={num}
                        label={num}
                        onDelete={() => handleDeleteNumber(num)}
                        deleteIcon={<DeleteIcon />}
                      />
                    ))}
                  </Box>
                ) : (
                  <Alert severity="warning">尚未添加白名单号码</Alert>
                )}
              </Box>

              <Divider />

              {/* Action List */}
              <Box>
                <Box sx={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', mb: 2 }}>
                  <Typography variant="subtitle2">
                    遥控动作列表
                  </Typography>
                  <Button
                    size="small"
                    startIcon={<AddIcon />}
                    onClick={() => {
                      if (callConfig.actions.length >= 10) return
                      setCallConfig({
                        ...callConfig,
                        actions: [...callConfig.actions, { hold_seconds: 15, action: 'reboot' }],
                      })
                    }}
                    disabled={callConfig.actions.length >= 10}
                  >
                    添加动作
                  </Button>
                </Box>
                {callConfig.actions.length > 0 ? (
                  <Stack spacing={2}>
                    {callConfig.actions
                      .map((a, i) => ({ ...a, _idx: i }))
                      .sort((a, b) => a.hold_seconds - b.hold_seconds)
                      .map((item) => (
                        <Box
                          key={item._idx}
                          sx={{
                            display: 'flex',
                            gap: 1,
                            alignItems: 'center',
                            p: 1.5,
                            border: '1px solid',
                            borderColor: 'divider',
                            borderRadius: 1,
                          }}
                        >
                          <TextField
                            type="number"
                            size="small"
                            label="等待(秒)"
                            value={item.hold_seconds}
                            onChange={(e) => {
                              const newActions = [...callConfig.actions]
                              newActions[item._idx] = {
                                ...newActions[item._idx],
                                hold_seconds: Math.max(5, Math.min(3600, parseInt(e.target.value) || 5)),
                              }
                              setCallConfig({ ...callConfig, actions: newActions })
                            }}
                            inputProps={{ min: 5, max: 3600 }}
                            sx={{ width: 110 }}
                          />
                          <FormControl size="small" sx={{ flex: 1 }}>
                            <Select
                              value={item.action}
                              onChange={(e) => {
                                const newActions = [...callConfig.actions]
                                newActions[item._idx] = {
                                  ...newActions[item._idx],
                                  action: e.target.value as ScheduleAction,
                                }
                                setCallConfig({ ...callConfig, actions: newActions })
                              }}
                            >
                              {Object.entries(ACTION_LABELS).map(([value, label]) => (
                                <MuiMenuItem key={value} value={value}>
                                  {label}
                                </MuiMenuItem>
                              ))}
                            </Select>
                          </FormControl>
                          <IconButton
                            size="small"
                            color="error"
                            onClick={() => {
                              setCallConfig({
                                ...callConfig,
                                actions: callConfig.actions.filter((_, i) => i !== item._idx),
                              })
                            }}
                          >
                            <DeleteIcon />
                          </IconButton>
                        </Box>
                      ))}
                  </Stack>
                ) : (
                  <Alert severity="warning">
                    尚未配置遥控动作，请点击"添加动作"创建。接通后所有动作同时计时，按等待时长依次触发。
                  </Alert>
                )}
                <Typography variant="caption" color="text.secondary" sx={{ mt: 1, display: 'block' }}>
                  最多 10 条动作。所有动作从接通时刻同时开始计时，互不干扰。
                  飞行模式相关动作会在 10 秒后自动恢复网络。
                </Typography>
              </Box>

              <Button
                variant="contained"
                startIcon={callLoading ? <CircularProgress size={20} /> : <SaveIcon />}
                onClick={() => void handleSaveCall()}
                disabled={callLoading}
              >
                {callLoading ? '保存中...' : '保存配置'}
              </Button>
            </Stack>
          )}
        </Paper>
      )}

      <Snackbar
        open={snackbar.open}
        autoHideDuration={3000}
        onClose={() => setSnackbar({ ...snackbar, open: false })}
        message={snackbar.message}
        anchorOrigin={{ vertical: 'bottom', horizontal: 'center' }}
      />
    </Box>
  )
}
