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
const PRESET_BROKERS = [
  { host: 'ssl://lafffe12.ala.cn-hangzhou.emqxsl.cn', port: 8883, label: 'EMQX 杭州 (TLS)' },
  { host: 'broker.emqx.io', port: 1883, label: 'EMQX 公共 (明文)' },
  { host: 'broker-cn.emqx.io', port: 1883, label: 'EMQX 中国 (明文)' },
  { host: 'ssl://broker.emqx.io', port: 8883, label: 'EMQX 公共 (TLS)' },
  { host: 'test.mosquitto.org', port: 1883, label: 'Mosquitto 测试 (明文)' },
  { host: 'ssl://test.mosquitto.org', port: 8883, label: 'Mosquitto 测试 (TLS)' },
  { host: 'mqtt.eclipseprojects.io', port: 1883, label: 'Eclipse IoT (明文)' },
  { host: 'ssl://mqtt.eclipseprojects.io', port: 8883, label: 'Eclipse IoT (TLS)' },
  { host: 'broker.hivemq.com', port: 1883, label: 'HiveMQ 公共 (明文)' },
]

import {
  Add as AddIcon,
  Delete as DeleteIcon,
  Save as SaveIcon,
  Sms as SmsIcon,
  Phone as PhoneIcon,
  Refresh as RefreshIcon,
  CloudQueue as CloudIcon,
  Notifications as NotificationsIcon,
  Send as SendIcon,
  Star as StarIcon,
  StarBorder as StarBorderIcon,
} from '@mui/icons-material'
import { api } from '../api'
import type {
  SmsControlConfigResponse,
  CallControlConfig,
  CallControlDurationCommand,
  CallControlTrigger,
  ScheduleAction,
  MqttConfigResponse,
  MqttStatusResponse,
  RemoteControlPushConfig,
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

  const activeTabMap: Record<string, number> = { '/remote/sms': 0, '/remote/call': 1, '/remote/mqtt': 2, '/remote/push': 3 }
  const activeTab = activeTabMap[location.pathname] ?? 0

  const handleTabChange = (_: unknown, newValue: number) => {
    const paths = ['/remote/sms', '/remote/call', '/remote/mqtt', '/remote/push']
    void navigate(paths[newValue] || '/remote/sms')
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
    duration_commands: [],
  })
  const [callTrigger, setCallTrigger] = useState<CallControlTrigger | null>(null)
  const [callLoading, setCallLoading] = useState(false)
  const [callInitialized, setCallInitialized] = useState(false)
  const [newNumber, setNewNumber] = useState('')

  // MQTT Control state
  const [mqttConfig, setMqttConfig] = useState<MqttConfigResponse>({
    enabled: false,
    broker_list: [
      'ssl://lafffe12.ala.cn-hangzhou.emqxsl.cn',
      'broker.emqx.io',
      'broker-cn.emqx.io',
    ],
    active_broker: 'ssl://lafffe12.ala.cn-hangzhou.emqxsl.cn',
    port: 8883,
    topic_sub: 'cpe/{imei}/cmd',
    topic_pub: 'cpe/{imei}/status',
    auth_token: null,
    tls: true,
    username: null,
    password: null,
  })
  const [mqttStatus, setMqttStatus] = useState<MqttStatusResponse>({
    enabled: false,
    connected: false,
    current_broker: '',
    last_heartbeat: null,
    last_command: null,
    error_message: null,
    broker_index: 0,
  })
  const [mqttConfigLoading, setMqttConfigLoading] = useState(false)
  const [mqttInitialized, setMqttInitialized] = useState(false)

  // Push Notification state
  const [pushConfig, setPushConfig] = useState<RemoteControlPushConfig>({
    enabled: false,
    webhook_url: '',
    headers: {},
    secret: '',
    template: `{
  "msgtype": "text",
  "text": {
    "content": "🔔 {{data_message}}\\n时间: {{timestamp}}"
  }
}`,
    forward_sms_control: true,
    forward_call_control: true,
    forward_mqtt_control: true,
  })
  const [pushLoading, setPushLoading] = useState(false)
  const [pushInitialized, setPushInitialized] = useState(false)
  const [pushTesting, setPushTesting] = useState(false)
  const [newPushHeaderKey, setNewPushHeaderKey] = useState('')
  const [newPushHeaderValue, setNewPushHeaderValue] = useState('')

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

  // Load MQTT config and status
  const loadMqttConfig = useCallback(async () => {
    try {
      const [configRes, statusRes] = await Promise.all([
        api.getMqttConfig(),
        api.getMqttStatus(),
      ])
      if (configRes.data) setMqttConfig(configRes.data)
      if (statusRes.data) setMqttStatus(statusRes.data)
    } catch {
      showSnackbar('加载 MQTT 配置失败', 'error')
    } finally {
      setMqttInitialized(true)
    }
  }, [])

  useEffect(() => {
    if (activeTab === 2 && !mqttInitialized) {
      void loadMqttConfig()
    }
  }, [activeTab, mqttInitialized, loadMqttConfig])

  // Poll MQTT status every 2 seconds when on MQTT tab
  const refreshMqttStatus = useCallback(async () => {
    try {
      const res = await api.getMqttStatus()
      if (res.data) setMqttStatus(res.data)
    } catch { /* ignore poll errors */ }
  }, [])

  useEffect(() => {
    if (activeTab !== 2) return
    // 立即拉一次
    void refreshMqttStatus()
    const timer = setInterval(() => { void refreshMqttStatus() }, 2000)
    return () => clearInterval(timer)
  }, [activeTab, refreshMqttStatus])

  // Load push config
  const loadPushConfig = useCallback(async () => {
    try {
      const res = await api.getRemoteControlPushConfig()
      if (res.data) setPushConfig(res.data)
    } catch {
      showSnackbar('加载推送配置失败', 'error')
    } finally {
      setPushInitialized(true)
    }
  }, [])

  useEffect(() => {
    if (activeTab === 3 && !pushInitialized) {
      void loadPushConfig()
    }
  }, [activeTab, pushInitialized, loadPushConfig])

  // Save push config
  const handleSavePush = async () => {
    setPushLoading(true)
    try {
      const res = await api.setRemoteControlPushConfig(pushConfig)
      if (res.data) {
        setPushConfig(res.data)
        showSnackbar('推送配置已保存', 'success')
      }
    } catch {
      showSnackbar('保存推送配置失败', 'error')
    } finally {
      setPushLoading(false)
    }
  }

  // Test push
  const handleTestPush = async () => {
    setPushTesting(true)
    try {
      await api.testRemoteControlPush()
      showSnackbar('测试消息已发送', 'success')
    } catch {
      showSnackbar('测试消息发送失败', 'error')
    } finally {
      setPushTesting(false)
    }
  }

  // Save MQTT config
  const handleSaveMqtt = async () => {
    setMqttConfigLoading(true)
    try {
      const res = await api.setMqttConfig(mqttConfig)
      if (res.data) {
        setMqttConfig(res.data)
        showSnackbar('MQTT 配置已保存，服务将自动重连', 'success')
        // 立即拉取一次状态，然后持续 2s 轮询等待连接结果
        void refreshMqttStatus()
        for (let i = 0; i < 5; i++) {
          await new Promise(r => setTimeout(r, 2000))
          void refreshMqttStatus()
        }
      }
    } catch {
      showSnackbar('保存 MQTT 配置失败', 'error')
    } finally {
      setMqttConfigLoading(false)
    }
  }

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
        <Tab icon={<CloudIcon />} iconPosition="start" label="MQTT" />
        <Tab icon={<NotificationsIcon />} iconPosition="start" label="推送通知" />
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
                <Typography variant="body2" fontWeight="bold" gutterBottom>
                  通话时长编码遥控
                </Typography>
                <Typography variant="body2" component="div">
                  <ol style={{ margin: 0, paddingLeft: 20 }}>
                    <li>白名单号码来电 → 自动接听并计时</li>
                    <li>保持通话 N 秒后挂断 → 根据时长匹配命令（±2秒容差）</li>
                    <li>推送通知："检测到命令，10秒内再次来电确认"</li>
                    <li>同号码10秒内再次来电 → 确认执行</li>
                    <li>超时未确认 → 自动取消</li>
                  </ol>
                </Typography>
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

              {/* Duration Commands List */}
              <Box>
                <Box sx={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', mb: 2 }}>
                  <Typography variant="subtitle2">
                    通话时长 → 命令映射
                  </Typography>
                  <Button
                    size="small"
                    startIcon={<AddIcon />}
                    onClick={() => {
                      if (callConfig.duration_commands.length >= 10) return
                      // Find next unused duration (5, 10, 15, ...)
                      const usedDurations = new Set(callConfig.duration_commands.map(c => c.duration_secs))
                      let nextDur = 5
                      while (usedDurations.has(nextDur)) nextDur += 5
                      const newCmd: CallControlDurationCommand = {
                        duration_secs: nextDur,
                        action: 'reboot',
                        label: '',
                      }
                      setCallConfig({
                        ...callConfig,
                        duration_commands: [...callConfig.duration_commands, newCmd],
                      })
                    }}
                    disabled={callConfig.duration_commands.length >= 10}
                  >
                    添加命令
                  </Button>
                </Box>
                {callConfig.duration_commands.length > 0 ? (
                  <Stack spacing={2}>
                    {callConfig.duration_commands
                      .map((cmd, i) => ({ ...cmd, _idx: i }))
                      .sort((a, b) => a.duration_secs - b.duration_secs)
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
                            label="通话时长(秒)"
                            value={item.duration_secs}
                            onChange={(e) => {
                              const newCmds = [...callConfig.duration_commands]
                              newCmds[item._idx] = {
                                ...newCmds[item._idx],
                                duration_secs: Math.max(3, Math.min(30, parseInt(e.target.value) || 3)),
                              }
                              setCallConfig({ ...callConfig, duration_commands: newCmds })
                            }}
                            inputProps={{ min: 3, max: 30 }}
                            sx={{ width: 130 }}
                          />
                          <FormControl size="small" sx={{ flex: 1 }}>
                            <Select
                              value={item.action}
                              onChange={(e) => {
                                const newCmds = [...callConfig.duration_commands]
                                newCmds[item._idx] = {
                                  ...newCmds[item._idx],
                                  action: e.target.value as ScheduleAction,
                                }
                                setCallConfig({ ...callConfig, duration_commands: newCmds })
                              }}
                            >
                              {Object.entries(ACTION_LABELS).map(([value, label]) => (
                                <MuiMenuItem key={value} value={value}>
                                  {label}
                                </MuiMenuItem>
                              ))}
                            </Select>
                          </FormControl>
                          <TextField
                            size="small"
                            label="标签(可选)"
                            value={item.label}
                            onChange={(e) => {
                              const newCmds = [...callConfig.duration_commands]
                              newCmds[item._idx] = {
                                ...newCmds[item._idx],
                                label: e.target.value,
                              }
                              setCallConfig({ ...callConfig, duration_commands: newCmds })
                            }}
                            placeholder="例: 重启"
                            sx={{ width: 100 }}
                          />
                          <IconButton
                            size="small"
                            color="error"
                            onClick={() => {
                              setCallConfig({
                                ...callConfig,
                                duration_commands: callConfig.duration_commands.filter((_, i) => i !== item._idx),
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
                    尚未配置命令映射。请点击"添加命令"创建时长→动作映射。
                  </Alert>
                )}
                <Typography variant="caption" color="text.secondary" sx={{ mt: 1, display: 'block' }}>
                  通话时长范围 3-30 秒，±2 秒容差匹配。每个时长只能配置一条命令，最多 10 条。
                  检测后需同号码 10 秒内再次来电确认执行。飞行模式相关动作会在 10 秒后自动恢复网络。
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

      {/* MQTT Remote Control Panel */}
      {activeTab === 2 && (
        <Paper sx={{ p: 3 }}>
          {!mqttInitialized ? (
            <Box sx={{ display: 'flex', justifyContent: 'center', p: 4 }}>
              <CircularProgress />
            </Box>
          ) : (
            <Stack spacing={3}>
              <Alert severity="info">
                基于 MQTT 协议的远程控制，适用于纯数据物联卡（无短信/通话权限）。
                设备通过公共 MQTT Broker 收发指令，支持多节点自动故障转移。
              </Alert>

              <FormControlLabel
                control={
                  <Switch
                    checked={mqttConfig.enabled}
                    onChange={(e) =>
                      setMqttConfig({ ...mqttConfig, enabled: e.target.checked })
                    }
                  />
                }
                label="启用 MQTT 远程控制"
              />

              {/* Connection Status Card */}
              <Card variant={mqttStatus.connected ? 'outlined' : 'outlined'}>
                <CardContent>
                  <Typography variant="subtitle1" fontWeight="bold" gutterBottom>
                    连接状态
                  </Typography>
                  <Stack spacing={1}>
                    <Box sx={{ display: 'flex', alignItems: 'center', gap: 1 }}>
                      <Box
                        sx={{
                          width: 10,
                          height: 10,
                          borderRadius: '50%',
                          backgroundColor: mqttStatus.connected ? '#4caf50' : '#f44336',
                        }}
                      />
                      <Typography variant="body2">
                        {mqttStatus.connected ? '已连接' : '未连接'}
                      </Typography>
                    </Box>
                    <Typography variant="body2">
                      当前 Broker: {mqttStatus.current_broker || '—'}
                    </Typography>
                    <Typography variant="body2">
                      最后心跳: {mqttStatus.last_heartbeat || '—'}
                    </Typography>
                    <Typography variant="body2">
                      最后指令: {mqttStatus.last_command || '—'}
                    </Typography>
                    {mqttStatus.error_message && (
                      <Alert severity="error" sx={{ mt: 1 }}>
                        {mqttStatus.error_message}
                      </Alert>
                    )}
                  </Stack>
                </CardContent>
              </Card>

              <Divider />

              {/* Broker Configuration */}
              <Box>
                <Typography variant="subtitle2" gutterBottom>
                  Broker 节点列表
                </Typography>
                <Alert severity="info" sx={{ mb: 2 }}>
                  <Typography variant="body2">
                    <strong>连接格式：</strong><br />
                    • 明文：<code>broker.emqx.io</code>（端口 1883）<br />
                    • TLS：<code>ssl://broker.emqx.io</code>（端口 8883）<br />
                    • 端口在下方统一配置，双击预设节点可设为当前激活
                  </Typography>
                </Alert>

                {/* Preset Brokers */}
                <Typography variant="caption" color="text.secondary" sx={{ mb: 1, display: 'block' }}>
                  公共服务（点击加入列表，已加入的再点移除）：
                </Typography>
                <Box sx={{ display: 'flex', flexWrap: 'wrap', gap: 0.5, mb: 2 }}>
                  {PRESET_BROKERS.map((pb) => {
                    const inList = mqttConfig.broker_list.includes(pb.host)
                    return (
                      <Chip
                        key={pb.host}
                        label={pb.label}
                        size="small"
                        color={inList ? 'primary' : 'default'}
                        variant={inList ? 'filled' : 'outlined'}
                        onClick={() => {
                          if (inList) {
                            const rest = mqttConfig.broker_list.filter(b => b !== pb.host)
                            setMqttConfig({
                              ...mqttConfig,
                              broker_list: rest.length ? rest : [''],
                              active_broker: mqttConfig.active_broker === pb.host ? (rest[0] ?? '') : mqttConfig.active_broker,
                            })
                          } else {
                            setMqttConfig({
                              ...mqttConfig,
                              broker_list: [...mqttConfig.broker_list.filter(b => b), pb.host],
                              active_broker: mqttConfig.active_broker || pb.host,
                              port: pb.port,
                              tls: pb.host.startsWith('ssl://'),
                            })
                          }
                        }}
                      />
                    )
                  })}
                </Box>

                <Typography variant="caption" color="text.secondary" sx={{ mb: 1, display: 'block' }}>
                  当前节点列表（★ = 当前激活，拖拽不可用请删除后重新添加以便排序）：
                </Typography>
                {mqttConfig.broker_list.map((broker, index) => {
                  const preset = PRESET_BROKERS.find(p => p.host === broker)
                  return (
                    <Box key={index} sx={{ display: 'flex', gap: 1, mb: 1, alignItems: 'center' }}>
                      {preset ? (
                        <Chip
                          label={`${preset.label}${broker === mqttConfig.active_broker ? ' ★' : ''}`}
                          size="small"
                          color={broker === mqttConfig.active_broker ? 'primary' : 'default'}
                          variant="filled"
                          sx={{ flex: 1, justifyContent: 'flex-start' }}
                        />
                      ) : (
                        <TextField
                          size="small"
                          value={broker}
                          onChange={(e) => {
                            const newList = [...mqttConfig.broker_list]
                            newList[index] = e.target.value
                            setMqttConfig({
                              ...mqttConfig,
                              broker_list: newList,
                              active_broker: mqttConfig.active_broker === broker ? e.target.value : mqttConfig.active_broker,
                            })
                          }}
                          sx={{ flex: 1 }}
                          placeholder="ssl://custom.broker.com"
                        />
                      )}
                      <IconButton
                        size="small"
                        color={broker === mqttConfig.active_broker ? 'primary' : 'default'}
                        onClick={() => setMqttConfig({ ...mqttConfig, active_broker: broker })}
                        title="设为当前激活节点"
                      >
                        {broker === mqttConfig.active_broker ? <StarIcon fontSize="small" /> : <StarBorderIcon fontSize="small" />}
                      </IconButton>
                      <IconButton
                        size="small"
                        color="error"
                        onClick={() => {
                          const rest = mqttConfig.broker_list.filter((_, i) => i !== index)
                          setMqttConfig({
                            ...mqttConfig,
                            broker_list: rest.length ? rest : [''],
                            active_broker: mqttConfig.active_broker === broker ? (rest[0] ?? '') : mqttConfig.active_broker,
                          })
                        }}
                        disabled={mqttConfig.broker_list.length <= 1}
                      >
                        <DeleteIcon fontSize="small" />
                      </IconButton>
                    </Box>
                  )
                })}
                <Button
                  size="small"
                  startIcon={<AddIcon />}
                  onClick={() =>
                    setMqttConfig({
                      ...mqttConfig,
                      broker_list: [...mqttConfig.broker_list.filter(b => b), ''],
                    })
                  }
                  sx={{ mt: 1 }}
                >
                  添加自定义节点
                </Button>
              </Box>

              <Box sx={{ display: 'flex', gap: 2 }}>
                <TextField
                  size="small"
                  label="端口"
                  type="number"
                  value={mqttConfig.port}
                  onChange={(e) =>
                    setMqttConfig({ ...mqttConfig, port: parseInt(e.target.value) || 1883 })
                  }
                  inputProps={{ min: 1, max: 65535 }}
                  sx={{ width: 120 }}
                />
                <TextField
                  size="small"
                  label="订阅主题 (接收指令)"
                  value={mqttConfig.topic_sub}
                  onChange={(e) =>
                    setMqttConfig({ ...mqttConfig, topic_sub: e.target.value })
                  }
                  sx={{ flex: 1 }}
                  helperText="使用 {imei} 作为设备占位符"
                />
              </Box>

              <TextField
                size="small"
                label="发布主题 (发送状态)"
                value={mqttConfig.topic_pub}
                onChange={(e) =>
                  setMqttConfig({ ...mqttConfig, topic_pub: e.target.value })
                }
                helperText="使用 {imei} 作为设备占位符"
              />

              <TextField
                size="small"
                label="鉴权 Token (可选)"
                value={mqttConfig.auth_token ?? ''}
                onChange={(e) =>
                  setMqttConfig({
                    ...mqttConfig,
                    auth_token: e.target.value || null,
                  })
                }
                helperText="设置后只有携带正确 token 的指令才会执行"
              />

              <Divider sx={{ my: 1 }} />
              <Typography variant="subtitle2">连接安全</Typography>

              <FormControlLabel
                control={
                  <Switch
                    checked={mqttConfig.tls}
                    onChange={(e) => {
                      const tls = e.target.checked
                      setMqttConfig({
                        ...mqttConfig,
                        tls,
                        port: tls ? 8883 : 1883,
                      })
                    }}
                  />
                }
                label="启用 TLS (SSL)"
              />

              <TextField
                size="small"
                label="用户名 (可选)"
                value={mqttConfig.username ?? ''}
                onChange={(e) =>
                  setMqttConfig({
                    ...mqttConfig,
                    username: e.target.value || null,
                  })
                }
                helperText="EMQX 等自建 Broker 认证用"
              />

              <TextField
                size="small"
                type="password"
                label="密码 (可选)"
                value={mqttConfig.password ?? ''}
                onChange={(e) =>
                  setMqttConfig({
                    ...mqttConfig,
                    password: e.target.value || null,
                  })
                }
                helperText="与用户名配合使用"
              />

              <Divider />

              <Alert severity="info">
                <Typography variant="subtitle2" gutterBottom>
                  支持的指令（通过 MQTT 发送 JSON 到订阅主题）：
                </Typography>
                <Box sx={{ display: 'flex', flexWrap: 'wrap', gap: 0.5, mt: 1 }}>
                  <Chip label='{"action":"reboot"}' size="small" color="primary" variant="outlined" />
                  <Chip label='{"action":"reconnect"}' size="small" color="primary" variant="outlined" />
                  <Chip label='{"action":"status"}' size="small" color="primary" variant="outlined" />
                </Box>
                <Typography variant="caption" color="text.secondary" sx={{ mt: 1, display: 'block' }}>
                  若配置了 Token，需添加 "token" 字段：{`{"action":"reboot","token":"your-token"}`}
                </Typography>
              </Alert>

              <Button
                variant="contained"
                startIcon={mqttConfigLoading ? <CircularProgress size={20} /> : <SaveIcon />}
                onClick={() => void handleSaveMqtt()}
                disabled={mqttConfigLoading}
              >
                {mqttConfigLoading ? '保存中...' : '保存配置'}
              </Button>
            </Stack>
          )}
        </Paper>
      )}

      {/* Push Notification Panel */}
      {activeTab === 3 && (
        <Paper sx={{ p: 3 }}>
          {!pushInitialized ? (
            <Box sx={{ display: 'flex', justifyContent: 'center', p: 4 }}>
              <CircularProgress />
            </Box>
          ) : (
            <Stack spacing={3}>
              <Alert severity="info">
                远程遥控推送通知独立配置。当通过短信/通话/MQTT 执行遥控指令时，
                设备会向此处配置的 Webhook 地址发送中文可读的推送通知。
              </Alert>

              <FormControlLabel
                control={
                  <Switch
                    checked={pushConfig.enabled}
                    onChange={(e) =>
                      setPushConfig({ ...pushConfig, enabled: e.target.checked })
                    }
                  />
                }
                label="启用远程遥控推送"
              />

              <TextField
                label="Webhook URL"
                value={pushConfig.webhook_url}
                onChange={(e) =>
                  setPushConfig({ ...pushConfig, webhook_url: e.target.value })
                }
                placeholder="https://your-server.com/webhook"
                helperText="支持任意 HTTP 服务器，推送 JSON 格式的中文通知"
                disabled={!pushConfig.enabled}
              />

              <TextField
                label="签名密钥（可选）"
                value={pushConfig.secret}
                onChange={(e) =>
                  setPushConfig({ ...pushConfig, secret: e.target.value })
                }
                placeholder="用于 HMAC-SHA256 签名"
                helperText="设置后会在 X-Signature 请求头中携带签名"
                disabled={!pushConfig.enabled}
              />

              <TextField
                label="推送模板"
                value={pushConfig.template}
                onChange={(e) =>
                  setPushConfig({ ...pushConfig, template: e.target.value })
                }
                multiline
                minRows={6}
                maxRows={12}
                placeholder='{"msg_type":"text","content":{"text":"..."}}'
                helperText='支持 {{timestamp}} {{type}} {{data_message}} {{data_event}} {{data_command}} 等变量'
                disabled={!pushConfig.enabled}
                sx={{ '& .MuiInputBase-root': { fontFamily: 'monospace', fontSize: '0.8rem' } }}
              />

              <Divider />

              <Box>
                <Typography variant="subtitle2" gutterBottom>
                  自定义请求头
                </Typography>
                <Box sx={{ display: 'flex', gap: 1, mb: 2 }}>
                  <TextField
                    size="small"
                    label="Header 名称"
                    value={newPushHeaderKey}
                    onChange={(e) => setNewPushHeaderKey(e.target.value)}
                    sx={{ flex: 1 }}
                    disabled={!pushConfig.enabled}
                  />
                  <TextField
                    size="small"
                    label="Header 值"
                    value={newPushHeaderValue}
                    onChange={(e) => setNewPushHeaderValue(e.target.value)}
                    sx={{ flex: 2 }}
                    disabled={!pushConfig.enabled}
                  />
                  <IconButton
                    color="primary"
                    onClick={() => {
                      const k = newPushHeaderKey.trim()
                      if (k) {
                        setPushConfig({
                          ...pushConfig,
                          headers: { ...pushConfig.headers, [k]: newPushHeaderValue },
                        })
                        setNewPushHeaderKey('')
                        setNewPushHeaderValue('')
                      }
                    }}
                    disabled={!pushConfig.enabled || !newPushHeaderKey.trim()}
                  >
                    <AddIcon />
                  </IconButton>
                </Box>
                {Object.keys(pushConfig.headers).length > 0 && (
                  <Box sx={{ display: 'flex', flexWrap: 'wrap', gap: 1 }}>
                    {Object.entries(pushConfig.headers).map(([key, value]) => (
                      <Chip
                        key={key}
                        label={`${key}: ${value}`}
                        onDelete={() => {
                          const newHeaders = { ...pushConfig.headers }
                          delete newHeaders[key]
                          setPushConfig({ ...pushConfig, headers: newHeaders })
                        }}
                        deleteIcon={<DeleteIcon />}
                        disabled={!pushConfig.enabled}
                      />
                    ))}
                  </Box>
                )}
              </Box>

              <Divider />

              <Box>
                <Typography variant="subtitle2" gutterBottom>
                  推送转发开关
                </Typography>
                <Typography variant="caption" color="text.secondary" sx={{ mb: 2, display: 'block' }}>
                  选择需要推送通知的遥控指令来源
                </Typography>
                <FormControlLabel
                  control={
                    <Switch
                      checked={pushConfig.forward_sms_control}
                      onChange={(e) =>
                        setPushConfig({ ...pushConfig, forward_sms_control: e.target.checked })
                      }
                      disabled={!pushConfig.enabled}
                    />
                  }
                  label="短信遥控通知"
                />
                <br />
                <FormControlLabel
                  control={
                    <Switch
                      checked={pushConfig.forward_call_control}
                      onChange={(e) =>
                        setPushConfig({ ...pushConfig, forward_call_control: e.target.checked })
                      }
                      disabled={!pushConfig.enabled}
                    />
                  }
                  label="通话遥控通知"
                />
                <br />
                <FormControlLabel
                  control={
                    <Switch
                      checked={pushConfig.forward_mqtt_control}
                      onChange={(e) =>
                        setPushConfig({ ...pushConfig, forward_mqtt_control: e.target.checked })
                      }
                      disabled={!pushConfig.enabled}
                    />
                  }
                  label="MQTT 遥控通知"
                />
              </Box>

              <Box sx={{ display: 'flex', gap: 2 }}>
                <Button
                  variant="contained"
                  startIcon={pushLoading ? <CircularProgress size={20} /> : <SaveIcon />}
                  onClick={() => void handleSavePush()}
                  disabled={pushLoading}
                >
                  {pushLoading ? '保存中...' : '保存配置'}
                </Button>
                <Button
                  variant="outlined"
                  startIcon={pushTesting ? <CircularProgress size={20} /> : <SendIcon />}
                  onClick={() => void handleTestPush()}
                  disabled={pushTesting || !pushConfig.enabled || !pushConfig.webhook_url}
                >
                  {pushTesting ? '测试中...' : '测试推送'}
                </Button>
              </Box>
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
