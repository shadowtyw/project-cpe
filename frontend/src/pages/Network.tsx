/*
 * @Author: 1orz cloudorzi@gmail.com
 * @Date: 2025-12-11 17:44:29
 * @LastEditors: 1orz cloudorzi@gmail.com
 * @LastEditTime: 2025-12-13 12:44:55
 * @FilePath: /udx710-backend/frontend/src/pages/Network.tsx
 * @Description: 
 * 
 * Copyright (c) 2025 by 1orz, All Rights Reserved. 
 */
import { useEffect, useState, type ChangeEvent } from 'react'
import {
  Box,
  Typography,
  Card,
  CardContent,
  CardHeader,
  Tab,
  Tabs,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  Paper,
  Chip,
  CircularProgress,
  Alert,
  Button,
  Snackbar,
  List,
  ListItem,
  ListItemText,
  ListItemSecondaryAction,
  Divider,
  Accordion,
  AccordionSummary,
  AccordionDetails,
  FormControlLabel,
  Checkbox,
  Switch,
  Stack,
  TextField,
  Select,
  MenuItem,
  FormControl,
  InputLabel,
} from '@mui/material'
import type { Theme } from '@mui/material/styles'
import {
  CellTower,
  Business,
  Search,
  Refresh,
  ExpandMore,
  MyLocation,
  ContentCopy,
  Lock,
  LockOpen,
  Router,
  Tune,
  Home,
  Public,
  Link as LinkIcon,
  Language,
  SimCard,
  NetworkCheck,
  SignalCellularAlt,
} from '@mui/icons-material'
import Grid from '@mui/material/Grid'
import { api, type RadioMode, type BandLockStatus, type BandLockRequest } from '../api'
import { useRefreshInterval } from '../contexts/RefreshContext'
import ErrorSnackbar from '../components/ErrorSnackbar'
import type { CellsResponse, OperatorListResponse, CellLocationResponse, CellLockStatusResponse, NetworkInterfaceInfo, IpAddress, ApnContext } from '../api/types'

// UDX710 设备支持的频段列表
const LTE_FDD_BANDS = [1, 3, 5, 8]
const LTE_TDD_BANDS = [39, 41]
const NR_FDD_BANDS = [1, 3, 28]
const NR_TDD_BANDS = [41, 77, 78, 79]

interface TabPanelProps {
  children?: React.ReactNode
  index: number
  value: number
}

function TabPanel(props: TabPanelProps) {
  const { children, value, index, ...other } = props
  return (
    <div
      role="tabpanel"
      hidden={value !== index}
      id={`network-tabpanel-${index}`}
      aria-labelledby={`network-tab-${index}`}
      {...other}
    >
      {value === index && <Box sx={{ pt: 3 }}>{children}</Box>}
    </div>
  )
}

export default function NetworkPage() {
  const { refreshInterval, refreshKey } = useRefreshInterval()
  const [initialLoading, setInitialLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)
  const [tabValue, setTabValue] = useState(0)
  
  const [cellsInfo, setCellsInfo] = useState<CellsResponse | null>(null)
  const [operators, setOperators] = useState<OperatorListResponse | null>(null)
  const [cellLocation, setCellLocation] = useState<CellLocationResponse | null>(null)
  const [scanning, setScanning] = useState(false)
  const [registering, setRegistering] = useState(false)
  
  // 小区锁定状态
  const [lockingCell, setLockingCell] = useState<string | null>(null) // 正在锁定的小区 key
  const [unlocking, setUnlocking] = useState(false)
  const [cellLockStatus, setCellLockStatus] = useState<CellLockStatusResponse | null>(null)
  
  // 网络接口状态
  const [interfaces, setInterfaces] = useState<NetworkInterfaceInfo[]>([])
  const [showDownInterfaces, setShowDownInterfaces] = useState(false)
  const [showIpAddresses, setShowIpAddresses] = useState(false)
  
  // 频段锁定状态
  const [currentRadioMode, setCurrentRadioMode] = useState<RadioMode>('auto')
  const [lockMode, setLockMode] = useState<'unlocked' | 'custom'>('unlocked') // 锁定模式
  const [lteFddBands, setLteFddBands] = useState<number[]>([])
  const [lteTddBands, setLteTddBands] = useState<number[]>([])
  const [nrFddBands, setNrFddBands] = useState<number[]>([])
  const [nrTddBands, setNrTddBands] = useState<number[]>([])
  const [_bandLockStatus, setBandLockStatus] = useState<BandLockStatus | null>(null)
  const [modeLoading, setModeLoading] = useState(false)
  const [bandLoading, setBandLoading] = useState(false)
  
  // APN 配置状态
  const [apnContexts, setApnContexts] = useState<ApnContext[]>([])
  const [selectedContext, setSelectedContext] = useState<string>('')
  const [apnForm, setApnForm] = useState({
    apn: '',
    protocol: 'dual',
    username: '',
    password: '',
    auth_method: 'chap',
  })
  const [apnSaving, setApnSaving] = useState(false)
  const [apnInitialized, setApnInitialized] = useState(false) // 控制 APN 只初始化一次
  
  // 频段配置刷新中
  const [bandConfigRefreshing, setBandConfigRefreshing] = useState(false)

  // 加载频段锁定配置（只在首次加载和手动刷新时调用，自动刷新不调用）
  const loadBandLockConfig = async () => {
    try {
      setBandConfigRefreshing(true)
      const [radioModeRes, bandLockRes] = await Promise.all([
        api.getRadioMode(),
        api.getBandLockStatus(),
      ])
      
      if (radioModeRes.data) {
        const mode = radioModeRes.data.mode
        if (mode === 'auto' || mode === 'lte' || mode === 'nr') {
          setCurrentRadioMode(mode as RadioMode)
        }
      }
      
      if (bandLockRes.data) {
        setBandLockStatus(bandLockRes.data)
        
        // 根据后端返回判断锁定模式
        // 后端逻辑：未锁定时返回空数组，已锁定时返回具体频段
        const isLocked = bandLockRes.data.locked
        const hasAnyBands = bandLockRes.data.lte_fdd_bands.length > 0 
                         || bandLockRes.data.lte_tdd_bands.length > 0
                         || bandLockRes.data.nr_fdd_bands.length > 0
                         || bandLockRes.data.nr_tdd_bands.length > 0
        
        if (!isLocked || !hasAnyBands) {
          // 未锁定模式
          setLockMode('unlocked')
          setLteFddBands([])
          setLteTddBands([])
          setNrFddBands([])
          setNrTddBands([])
        } else {
          // 自定义锁定模式
          setLockMode('custom')
          setLteFddBands(bandLockRes.data.lte_fdd_bands)
          setLteTddBands(bandLockRes.data.lte_tdd_bands)
          setNrFddBands(bandLockRes.data.nr_fdd_bands)
          setNrTddBands(bandLockRes.data.nr_tdd_bands)
        }
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBandConfigRefreshing(false)
    }
  }

  // 加载其他数据（自动刷新时调用，不包含频段锁定配置）
  const loadData = async () => {
    setError(null)
    
    try {
      const [cellsRes, operatorsRes, cellLocRes, cellLockRes, interfacesRes, apnRes] = await Promise.all([
        api.getCellsInfo(),
        api.getOperators(),
        api.getCellLocationInfo(),
        api.getCellLockStatus(),
        api.getNetworkInterfaces(),
        api.getApnList(),
      ])
      
      if (cellsRes.data) setCellsInfo(cellsRes.data)
      if (operatorsRes.data) setOperators(operatorsRes.data)
      if (cellLocRes.data) setCellLocation(cellLocRes.data)
      if (cellLockRes.data) setCellLockStatus(cellLockRes.data)
      if (interfacesRes.data) setInterfaces(interfacesRes.data.interfaces)
      
      if (apnRes.data?.contexts) {
        setApnContexts(apnRes.data.contexts)
        // 只在首次加载时初始化 APN 表单，避免覆盖用户输入
        if (!apnInitialized) {
          const activeContext = apnRes.data.contexts.find(c => c.apn) || apnRes.data.contexts[0]
          if (activeContext) {
            setSelectedContext(activeContext.path)
            setApnForm({
              apn: activeContext.apn,
              protocol: activeContext.protocol,
              username: activeContext.username,
              password: activeContext.password,
              auth_method: activeContext.auth_method,
            })
          }
          setApnInitialized(true)
        }
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setInitialLoading(false)
    }
  }

  // 首次加载：加载所有数据（包括频段配置）
  const loadAllData = async () => {
    await Promise.all([
      loadData(),
      loadBandLockConfig(),
    ])
  }

  // 手动刷新频段配置
  const handleRefreshBandConfig = () => {
    void loadBandLockConfig()
  }

  // 扫描运营商
  const scanOperators = async () => {
    setScanning(true)
    setError(null)
    try {
      const response = await api.scanOperators()
      if (response.status === 'ok' && response.data) {
        setOperators(response.data)
        setSuccess(`扫描完成，找到 ${response.data.operators.length} 个运营商`)
      } else {
        setError(response.message || '扫描失败')
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setScanning(false)
    }
  }

  const handleScanOperators = () => {
    void scanOperators()
  }

  // 手动注册运营商
  const registerManual = async (mccmnc: string) => {
    setRegistering(true)
    setError(null)
    try {
      const response = await api.registerOperatorManual(mccmnc)
      if (response.status === 'ok') {
        setSuccess(`正在注册到运营商 ${mccmnc}...`)
        setTimeout(loadData, 3000)
      } else {
        setError(response.message || '注册失败')
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setRegistering(false)
    }
  }

  const handleRegisterManual = (mccmnc: string) => {
    void registerManual(mccmnc)
  }

  // 自动注册
  const registerAuto = async () => {
    setRegistering(true)
    setError(null)
    try {
      const response = await api.registerOperatorAuto()
      if (response.status === 'ok') {
        setSuccess('已启动自动注册...')
        setTimeout(loadData, 3000)
      } else {
        setError(response.message || '自动注册失败')
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setRegistering(false)
    }
  }

  const handleRegisterAuto = () => {
    void registerAuto()
  }

  // 获取所有定位小区（主小区 + 邻区）
  const getAllLocationCells = () => {
    if (!cellLocation?.available) return []
    const cells: typeof cellLocation.neighbor_cells = []
    if (cellLocation.cell_info) {
      cells.push(cellLocation.cell_info)
    }
    cells.push(...cellLocation.neighbor_cells)
    return cells
  }

  // 复制基站定位参数
  const handleCopyCellLocation = () => {
    const cells = getAllLocationCells()
    if (!cells.length) return
    const cell = cells[0]
    const text = JSON.stringify(cell, null, 2)
    void navigator.clipboard.writeText(text)
    setSuccess('已复制基站定位参数到剪贴板')
  }

  // 锁定小区
  const handleLockCell = async (tech: string, arfcn: string, pci: string) => {
    const cellKey = `${tech}-${arfcn}-${pci}`
    setLockingCell(cellKey)
    setError(null)
    
    try {
      // 确定 RAT 类型：12=LTE, 16=NR
      const rat = tech.toLowerCase() === 'nr' || tech === 'NR' ? 16 : 12
      const arfcnNum = parseInt(arfcn, 10)
      const pciNum = parseInt(pci, 10)
      
      if (isNaN(arfcnNum) || isNaN(pciNum)) {
        setError('无效的频点或 PCI 值')
        return
      }
      
      const result = await api.setCellLock({
        rat,
        enable: true,
        arfcn: arfcnNum,
        pci: pciNum,
      })
      
      if (result.status === 'ok') {
        setSuccess(`已锁定到 ${tech.toUpperCase()} 小区 (ARFCN=${arfcn}, PCI=${pci})`)
        // 刷新数据
        setTimeout(() => void loadData(), 2000)
      } else {
        setError(result.message || '锁定失败')
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLockingCell(null)
    }
  }

  // 解锁所有小区
  const handleUnlockAllCells = async () => {
    setUnlocking(true)
    setError(null)
    
    try {
      const result = await api.unlockAllCells()
      if (result.status === 'ok') {
        setSuccess('已解除所有小区锁定')
        // 刷新数据
        setTimeout(() => void loadData(), 2000)
      } else {
        setError(result.message || '解锁失败')
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setUnlocking(false)
    }
  }

  // 切换射频模式
  const handleRadioModeChange = async (mode: RadioMode) => {
    setModeLoading(true)
    setError(null)
    try {
      const response = await api.setRadioMode(mode)
      setSuccess(response.message || '射频模式已切换')
      setCurrentRadioMode(mode)
      // 3秒后刷新频段配置（不影响用户正在编辑的其他内容）
      setTimeout(() => void loadBandLockConfig(), 3000)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setModeLoading(false)
    }
  }

  // 应用频段锁定
  const handleApplyBandLock = async () => {
    setBandLoading(true)
    setError(null)
    
    // 根据锁定模式构造请求
    const request: BandLockRequest = lockMode === 'unlocked' 
      ? {
          // 未锁定模式：发送空数组，解除所有限制
          lte_fdd_bands: [],
          lte_tdd_bands: [],
          nr_fdd_bands: [],
          nr_tdd_bands: [],
        }
      : {
          // 自定义锁定模式：发送用户选择的频段
          lte_fdd_bands: lteFddBands,
          lte_tdd_bands: lteTddBands,
          nr_fdd_bands: nrFddBands,
          nr_tdd_bands: nrTddBands,
        }
    
    try {
      const response = await api.setBandLock(request)
      setSuccess(response.message || '频段锁定配置已应用')
      // 1秒后刷新频段锁定状态
      setTimeout(() => void loadBandLockConfig(), 1000)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBandLoading(false)
    }
  }

  // 解除所有频段锁定
  const handleUnlockAllBands = async () => {
    setBandLoading(true)
    setError(null)
    const request: BandLockRequest = {
      lte_fdd_bands: [],
      lte_tdd_bands: [],
      nr_fdd_bands: [],
      nr_tdd_bands: [],
    }
    try {
      const response = await api.setBandLock(request)
      setSuccess(response.message || '频段限制已取消，所有频段可用')
      // 清空本地复选框状态
      setLteFddBands([])
      setLteTddBands([])
      setNrFddBands([])
      setNrTddBands([])
      // 1秒后刷新频段锁定状态
      setTimeout(() => void loadBandLockConfig(), 1000)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBandLoading(false)
    }
  }

  // 切换频段选择
  const toggleBand = (band: number, setter: React.Dispatch<React.SetStateAction<number[]>>) => {
    setter((prev) => (prev.includes(band) ? prev.filter((b) => b !== band) : [...prev, band]))
  }

  // APN 选择变更
  const handleContextChange = (path: string) => {
    setSelectedContext(path)
    const context = apnContexts.find(c => c.path === path)
    if (context) {
      setApnForm({
        apn: context.apn,
        protocol: context.protocol,
        username: context.username,
        password: context.password,
        auth_method: context.auth_method,
      })
    }
  }

  // 保存 APN 配置
  const saveApn = async () => {
    if (!selectedContext) {
      setError('请选择一个 APN 配置')
      return
    }
    
    try {
      setError(null)
      setSuccess(null)
      setApnSaving(true)
      
      await api.setApn({
        context_path: selectedContext,
        apn: apnForm.apn || undefined,
        protocol: apnForm.protocol || undefined,
        username: apnForm.username || undefined,
        password: apnForm.password || undefined,
        auth_method: apnForm.auth_method || undefined,
      })
      
      setSuccess('APN 配置已保存')
      setTimeout(() => { void loadData() }, 1000)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setApnSaving(false)
    }
  }

  // 获取协议显示名称
  const getProtocolName = (protocol: string) => {
    switch (protocol) {
      case 'ip': return 'IPv4'
      case 'ipv6': return 'IPv6'
      case 'dual': return 'IPv4v6'
      default: return protocol
    }
  }

  // 网络接口相关工具函数
  const formatBytes = (bytes: number): string => {
    if (bytes === 0) return '0 B'
    const k = 1024
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
    const i = Math.floor(Math.log(bytes) / Math.log(k))
    return `${(bytes / Math.pow(k, i)).toFixed(2)} ${sizes[i]}`
  }

  const getInterfaceStatusColor = (status: string) => {
    switch (status.toLowerCase()) {
      case 'up': return 'success'
      case 'down': return 'error'
      default: return 'warning'
    }
  }

  const getScopeIcon = (scope: string) => {
    switch (scope.toLowerCase()) {
      case 'public': return <Public fontSize="small" />
      case 'private': return <Home fontSize="small" />
      case 'loopback': return <Lock fontSize="small" />
      case 'link-local': return <LinkIcon fontSize="small" />
      default: return <Language fontSize="small" />
    }
  }

  const getScopeColor = (scope: string) => {
    switch (scope.toLowerCase()) {
      case 'public': return 'success'
      case 'private': return 'primary'
      case 'loopback': return 'default'
      case 'link-local': return 'warning'
      default: return 'default'
    }
  }

  const getScopeLabel = (scope: string) => {
    switch (scope.toLowerCase()) {
      case 'public': return '公网'
      case 'private': return '内网'
      case 'loopback': return '回环'
      case 'link-local': return '链路本地'
      default: return scope
    }
  }

  const getIpAddressStyle = () => ({
    filter: showIpAddresses ? 'none' : 'blur(5px)',
    transition: 'filter 0.3s ease',
    userSelect: showIpAddresses ? 'auto' : 'none',
    cursor: showIpAddresses ? 'text' : 'default',
  } as const)

  const filteredInterfaces = showDownInterfaces
    ? interfaces
    : interfaces.filter((iface) => iface.status.toLowerCase() !== 'down')

  useEffect(() => {
    // 首次加载：加载所有数据（包括频段配置）
    void loadAllData()
    
    // 自动刷新：只刷新小区、运营商等数据，不刷新频段配置（避免覆盖用户选择）
    if (refreshInterval > 0) {
      const interval = setInterval(() => {
        void loadData()
      }, refreshInterval)
      return () => clearInterval(interval)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshInterval, refreshKey])

  const handleTabChange = (_event: React.SyntheticEvent, newValue: number) => {
    setTabValue(newValue)
  }

  // 转换信号值
  const convertSignalValue = (value: string | number | undefined): number | null => {
    if (value === undefined || value === null) return null
    const numValue = typeof value === 'string' ? parseFloat(value) : value
    if (isNaN(numValue)) return null
    return numValue / 100
  }

  const formatSignalValue = (value: string | number | undefined): string => {
    const converted = convertSignalValue(value)
    if (converted === null) return '-'
    return converted.toFixed(2)
  }

  const getSignalChipColor = (rsrp?: string | number, rssi?: string | number) => {
    const rsrpValue = convertSignalValue(rsrp)
    const rssiValue = convertSignalValue(rssi)
    const value = rsrpValue || rssiValue || -120
    if (value >= -80) return 'success'
    if (value >= -100) return 'primary'
    if (value >= -110) return 'warning'
    return 'error'
  }

  if (initialLoading) {
    return (
      <Box display="flex" justifyContent="center" alignItems="center" minHeight="60vh">
        <CircularProgress />
      </Box>
    )
  }

  return (
    <Box>
      {/* 错误/成功提示 */}
      <ErrorSnackbar error={error} onClose={() => setError(null)} />
      <Snackbar open={!!success} autoHideDuration={3000} onClose={() => setSuccess(null)} anchorOrigin={{ vertical: 'top', horizontal: 'center' }}>
        <Alert severity="success" variant="filled" onClose={() => setSuccess(null)}>{success}</Alert>
      </Snackbar>

      {/* 页面标题 */}
      <Box mb={3}>
        <Typography variant="h4" gutterBottom fontWeight={600}>
          网络状态
        </Typography>
        <Typography variant="body2" color="text.secondary">
          查看网络信息、运营商、小区数据和 QoS 参数
        </Typography>
      </Box>

      {/* Tabs 导航 */}
      <Box sx={{ borderBottom: 1, borderColor: 'divider', mb: 2 }}>
        <Tabs value={tabValue} onChange={handleTabChange} variant="scrollable" scrollButtons="auto">
          <Tab label="小区与锁定" icon={<CellTower />} iconPosition="start" />
          <Tab label="APN 配置" icon={<SimCard />} iconPosition="start" />
          <Tab label="网络接口" icon={<Router />} iconPosition="start" />
          <Tab label="运营商管理" icon={<Business />} iconPosition="start" />
        </Tabs>
      </Box>

      {/* Tab 4: 运营商管理 */}
      <TabPanel value={tabValue} index={3}>
        <Grid container spacing={3}>
          <Grid size={{ xs: 12, md: 6 }}>
            <Card>
              <CardHeader
                avatar={<Business color="primary" />}
                title="运营商列表"
                titleTypographyProps={{ variant: 'h6' }}
                action={
                  <Box display="flex" gap={1}>
                    <Button
                      variant="outlined"
                      size="small"
                      startIcon={<Refresh />}
                      onClick={() => void loadData()}
                    >
                      刷新
                    </Button>
                  </Box>
                }
              />
              <CardContent>
                {operators?.operators?.length ? (
                  <List>
                    {operators.operators.map((op, idx) => (
                      <ListItem key={idx} divider>
                        <ListItemText
                          primary={
                            <Box display="flex" alignItems="center" gap={1}>
                              <Typography fontWeight={600}>{op.name}</Typography>
                              <Chip
                                label={op.status}
                                size="small"
                                color={op.status === 'current' ? 'success' : op.status === 'available' ? 'primary' : 'default'}
                              />
                            </Box>
                          }
                          secondary={
                            <>
                              <Typography variant="caption" display="block">
                                MCC-MNC: {op.mcc}-{op.mnc}
                              </Typography>
                              <Typography variant="caption" display="block">
                                技术: {op.technologies?.join(', ') || 'N/A'}
                              </Typography>
                            </>
                          }
                        />
                        <ListItemSecondaryAction>
                          {op.status !== 'current' && op.status !== 'forbidden' && (
                            <Button
                              size="small"
                              variant="outlined"
                              onClick={() => { handleRegisterManual(`${op.mcc}${op.mnc}`) }}
                              disabled={registering}
                            >
                              注册
                            </Button>
                          )}
                        </ListItemSecondaryAction>
                      </ListItem>
                    ))}
                  </List>
                ) : (
                  <Alert severity="info">暂无运营商数据</Alert>
                )}
              </CardContent>
            </Card>
          </Grid>

          <Grid size={{ xs: 12, md: 6 }}>
            <Card>
              <CardHeader
                avatar={<Search color="primary" />}
                title="运营商扫描"
                titleTypographyProps={{ variant: 'h6' }}
              />
              <CardContent>
                <Alert severity="warning" sx={{ mb: 2 }}>
                  扫描运营商需要约 <strong>2 分钟</strong>，期间网络可能不可用
                </Alert>
                <Button
                  variant="contained"
                  fullWidth
                  startIcon={scanning ? <CircularProgress size={20} color="inherit" /> : <Search />}
                  onClick={() => handleScanOperators()}
                  disabled={scanning}
                  sx={{ mb: 2 }}
                >
                  {scanning ? '正在扫描...' : '扫描可用运营商'}
                </Button>
                <Divider sx={{ my: 2 }} />
                <Button
                  variant="outlined"
                  fullWidth
                  startIcon={registering ? <CircularProgress size={20} /> : <Refresh />}
                  onClick={handleRegisterAuto}
                  disabled={registering}
                >
                  {registering ? '正在注册...' : '自动注册运营商'}
                </Button>
              </CardContent>
            </Card>
          </Grid>
        </Grid>
      </TabPanel>

      {/* Tab 1: 小区与锁定 */}
      <TabPanel value={tabValue} index={0}>
        {/* 锁定状态提示 */}
        {cellLockStatus?.any_locked && (
          <Alert 
            severity="warning" 
            sx={{ mb: 2 }}
            icon={<Lock fontSize="small" />}
            action={
              <Button
                color="inherit"
                size="small"
                startIcon={unlocking ? <CircularProgress size={14} /> : <LockOpen />}
                onClick={() => void handleUnlockAllCells()}
                disabled={unlocking}
              >
                解锁
              </Button>
            }
          >
            {cellLockStatus.rat_status.filter(s => s.enabled).map((status, idx) => (
              <Typography key={idx} variant="caption">
                {status.rat_name}: ARFCN={status.arfcn}, PCI={status.pci}
              </Typography>
            ))}
          </Alert>
        )}
        
        <Card>
          <Box sx={{ p: 1.5, display: 'flex', alignItems: 'center', justifyContent: 'space-between', flexWrap: 'wrap', gap: 1 }}>
            <Box display="flex" alignItems="center" gap={1} flexWrap="wrap">
              <CellTower fontSize="small" color="primary" />
              <Typography variant="subtitle2" fontWeight="medium">小区列表</Typography>
              {cellsInfo?.cells && (
                <Chip label={`${cellsInfo.cells.length}`} size="small" color="primary" variant="outlined" />
              )}
            </Box>
            <Button
              variant="outlined"
              color="warning"
              size="small"
              startIcon={unlocking ? <CircularProgress size={14} /> : <LockOpen />}
              onClick={() => void handleUnlockAllCells()}
              disabled={unlocking}
              sx={{ fontSize: '0.75rem', py: 0.5 }}
            >
              {unlocking ? '解锁中...' : '解除锁定'}
            </Button>
          </Box>
          
          <CardContent sx={{ pt: 0, px: { xs: 1, sm: 2 } }}>
            {/* Serving Cell 摘要 */}
            {cellsInfo?.serving_cell && (
              <Box 
                sx={{ 
                  display: 'flex', 
                  flexWrap: 'wrap', 
                  gap: 1, 
                  mb: 1.5, 
                  p: 1, 
                  bgcolor: 'action.hover', 
                  borderRadius: 1,
                }}
              >
                <Chip label={cellsInfo.serving_cell.tech?.toUpperCase() || '-'} size="small" color="primary" />
                <Typography variant="caption" color="text.secondary" sx={{ display: 'flex', alignItems: 'center', gap: 0.5 }}>
                  CID: <strong>{cellsInfo.serving_cell.cell_id}</strong>
                </Typography>
                <Typography variant="caption" color="text.secondary" sx={{ display: 'flex', alignItems: 'center', gap: 0.5 }}>
                  TAC: <strong>{cellsInfo.serving_cell.tac}</strong>
                </Typography>
              </Box>
            )}
            
            <TableContainer component={Paper} variant="outlined" sx={{ maxHeight: { xs: 350, sm: 400 } }}>
              <Table size="small" stickyHeader>
                <TableHead>
                  <TableRow>
                    <TableCell sx={{ py: 0.5, px: 1, fontSize: '0.7rem', minWidth: 55 }}>频段</TableCell>
                    <TableCell align="right" sx={{ py: 0.5, px: 0.5, fontSize: '0.7rem', minWidth: 55 }}>ARFCN</TableCell>
                    <TableCell align="right" sx={{ py: 0.5, px: 0.5, fontSize: '0.7rem', minWidth: 40 }}>PCI</TableCell>
                    <TableCell align="right" sx={{ py: 0.5, px: 0.5, fontSize: '0.7rem', minWidth: 50 }}>RSRP</TableCell>
                    <TableCell align="right" sx={{ py: 0.5, px: 0.5, fontSize: '0.7rem', minWidth: 45, display: { xs: 'none', sm: 'table-cell' } }}>RSRQ</TableCell>
                    <TableCell align="right" sx={{ py: 0.5, px: 0.5, fontSize: '0.7rem', minWidth: 45, display: { xs: 'none', sm: 'table-cell' } }}>SINR</TableCell>
                    <TableCell align="center" sx={{ py: 0.5, px: 0.5, fontSize: '0.7rem', minWidth: 60 }}>锁定</TableCell>
                  </TableRow>
                </TableHead>
                <TableBody>
                  {cellsInfo?.cells && cellsInfo.cells.length > 0 ? (
                    cellsInfo.cells.map((cell, idx) => {
                      // 判断该小区是否被锁定
                      const cellArfcn = Number(cell.arfcn || cell.earfcn || cell.nrarfcn || 0)
                      const cellPci = Number(cell.pci || 0)
                      const cellTech = cell.tech || (cell.type === 'NR' ? 'nr' : 'lte')
                      const isLocked = cellLockStatus?.rat_status.some(
                        s => s.enabled && 
                             s.arfcn === cellArfcn && 
                             s.pci === cellPci &&
                             ((cellTech.toLowerCase() === 'nr' && s.rat === 16) ||
                              (cellTech.toLowerCase() !== 'nr' && s.rat === 12))
                      )
                      
                      return (
                      <TableRow 
                        key={idx} 
                        sx={{ 
                          bgcolor: isLocked 
                            ? (theme: Theme) => theme.palette.mode === 'dark' ? 'rgba(237, 108, 2, 0.15)' : 'warning.light'
                            : cell.is_serving 
                              ? (theme: Theme) => theme.palette.mode === 'dark' ? 'rgba(102, 187, 106, 0.15)' : 'rgba(102, 187, 106, 0.08)'
                              : 'inherit',
                        }}
                      >
                        <TableCell sx={{ py: 0.5, px: 1 }}>
                          <Box display="flex" alignItems="center" gap={0.5}>
                            {isLocked ? (
                              <Lock sx={{ width: 10, height: 10, color: 'warning.main' }} />
                            ) : cell.is_serving ? (
                              <Box sx={{ width: 6, height: 6, borderRadius: '50%', bgcolor: 'success.main', flexShrink: 0 }} />
                            ) : null}
                            <Typography variant="caption" sx={{ fontSize: '0.75rem', fontWeight: cell.is_serving ? 600 : 400 }}>
                              {cell.band && cell.band !== '0' ? (
                                cell.band || '-'
                              ) : '-'}
                            </Typography>
                          </Box>
                        </TableCell>
                        <TableCell align="right" sx={{ py: 0.5, px: 0.5, fontSize: '0.75rem', fontFamily: 'monospace' }}>
                          {cell.arfcn || cell.earfcn || cell.nrarfcn || '-'}
                        </TableCell>
                        <TableCell align="right" sx={{ py: 0.5, px: 0.5, fontSize: '0.75rem', fontFamily: 'monospace' }}>
                          {cell.pci || '-'}
                        </TableCell>
                        <TableCell align="right" sx={{ py: 0.5, px: 0.5 }}>
                          {cell.rsrp !== undefined ? (
                            <Chip
                              label={formatSignalValue(cell.rsrp)}
                              size="small"
                              color={getSignalChipColor(cell.rsrp)}
                              sx={{ height: 18, fontSize: '0.65rem', '& .MuiChip-label': { px: 0.5 } }}
                            />
                          ) : cell.ssb_rsrp !== undefined ? (
                            <Chip
                              label={formatSignalValue(cell.ssb_rsrp)}
                              size="small"
                              color={getSignalChipColor(cell.ssb_rsrp)}
                              sx={{ height: 18, fontSize: '0.65rem', '& .MuiChip-label': { px: 0.5 } }}
                            />
                          ) : '-'}
                        </TableCell>
                        <TableCell align="right" sx={{ py: 0.5, px: 0.5, fontSize: '0.7rem', fontFamily: 'monospace', display: { xs: 'none', sm: 'table-cell' } }}>
                          {cell.rsrq !== undefined ? formatSignalValue(cell.rsrq) : cell.ssb_rsrq !== undefined ? formatSignalValue(cell.ssb_rsrq) : '-'}
                        </TableCell>
                        <TableCell align="right" sx={{ py: 0.5, px: 0.5, fontSize: '0.7rem', fontFamily: 'monospace', display: { xs: 'none', sm: 'table-cell' } }}>
                          {cell.sinr !== undefined ? formatSignalValue(cell.sinr) : cell.ssb_sinr !== undefined ? formatSignalValue(cell.ssb_sinr) : '-'}
                        </TableCell>
                        <TableCell align="center" sx={{ py: 0.5, px: 0.5 }}>
                          {(() => {
                            const arfcn = String(cell.arfcn || cell.earfcn || cell.nrarfcn || '')
                            const pci = String(cell.pci || '')
                            const tech = cell.tech || (cell.type === 'NR' ? 'nr' : 'lte')
                            const cellKey = `${tech}-${arfcn}-${pci}`
                            const isLocking = lockingCell === cellKey
                            
                            if (!arfcn || !pci) return '-'
                            
                            return (
                              <Button
                                size="small"
                                variant={isLocked ? 'contained' : 'text'}
                                color={isLocked ? 'warning' : 'primary'}
                                onClick={() => isLocked ? void handleUnlockAllCells() : void handleLockCell(tech, arfcn, pci)}
                                disabled={isLocking || !!lockingCell || unlocking}
                                sx={{ minWidth: 40, p: 0.5, fontSize: '0.7rem' }}
                              >
                                {isLocking ? '锁定中' : (isLocked ? '解锁' : '锁定')}
                              </Button>
                            )
                          })()}
                        </TableCell>
                      </TableRow>
                    )})
                  ) : (
                    <TableRow>
                      <TableCell colSpan={7} align="center" sx={{ py: 2 }}>
                        <Typography variant="caption" color="text.secondary">暂无小区数据</Typography>
                      </TableCell>
                    </TableRow>
                  )}
                </TableBody>
              </Table>
            </TableContainer>
          </CardContent>
        </Card>

        {/* 频段锁定配置 */}
        <Card sx={{ mt: 2 }}>
          <Box sx={{ p: 1.5, display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
            <Box display="flex" alignItems="center" gap={1}>
              <Tune fontSize="small" color="primary" />
              <Typography variant="subtitle2" fontWeight="medium">频段锁定配置</Typography>
            </Box>
            <Button
              size="small"
              variant="text"
              startIcon={bandConfigRefreshing ? <CircularProgress size={14} /> : <Refresh />}
              onClick={handleRefreshBandConfig}
              disabled={bandConfigRefreshing}
              sx={{ minWidth: 'auto', fontSize: '0.75rem' }}
            >
              刷新
            </Button>
          </Box>
          <CardContent sx={{ pt: 0, px: { xs: 1.5, sm: 2 } }}>
            {/* 射频模式切换 */}
            <Box mb={2}>
              <Typography variant="caption" color="text.secondary" gutterBottom display="block">射频模式</Typography>
              <Stack direction="row" spacing={0.5} flexWrap="wrap" useFlexGap>
                <Chip
                  label="Auto"
                  size="small"
                  color={currentRadioMode === 'auto' ? 'primary' : 'default'}
                  onClick={() => void handleRadioModeChange('auto')}
                  disabled={modeLoading}
                />
                <Chip
                  label="LTE"
                  size="small"
                  color={currentRadioMode === 'lte' ? 'primary' : 'default'}
                  onClick={() => void handleRadioModeChange('lte')}
                  disabled={modeLoading}
                />
                <Chip
                  label="NR"
                  size="small"
                  color={currentRadioMode === 'nr' ? 'primary' : 'default'}
                  onClick={() => void handleRadioModeChange('nr')}
                  disabled={modeLoading}
                />
                {modeLoading && <CircularProgress size={16} />}
              </Stack>
            </Box>

            <Divider sx={{ my: 1.5 }} />

            {/* 锁定模式选择 */}
            <Box mb={2}>
              <Typography variant="caption" color="text.secondary" gutterBottom display="block">
                锁定模式
              </Typography>
              <Stack direction="row" spacing={1} flexWrap="wrap" useFlexGap>
                <Chip
                  label="未锁定（使用所有频段）"
                  size="small"
                  color={lockMode === 'unlocked' ? 'success' : 'default'}
                  onClick={() => setLockMode('unlocked')}
                  disabled={bandLoading}
                  icon={lockMode === 'unlocked' ? <LockOpen /> : undefined}
                />
                <Chip
                  label="自定义锁定（选择允许的频段）"
                  size="small"
                  color={lockMode === 'custom' ? 'warning' : 'default'}
                  onClick={() => setLockMode('custom')}
                  disabled={bandLoading}
                  icon={lockMode === 'custom' ? <Lock /> : undefined}
                />
              </Stack>
            </Box>

            <Divider sx={{ my: 1.5 }} />

            {/* 频段选择区域 - 只在自定义锁定模式下显示 */}
            {lockMode === 'custom' && (
            <Grid container spacing={1.5}>
              {/* LTE FDD 频段 */}
              <Grid size={{ xs: 6, sm: 3 }}>
                <Typography variant="caption" color="text.secondary" gutterBottom display="block">LTE FDD (允许)</Typography>
                <Box sx={{ display: 'flex', flexWrap: 'wrap', gap: 0 }}>
                  {LTE_FDD_BANDS.map((band) => (
                    <FormControlLabel
                      key={`lte-fdd-${band}`}
                      control={
                        <Checkbox
                          checked={lteFddBands.includes(band)}
                          onChange={() => toggleBand(band, setLteFddBands)}
                          size="small"
                          sx={{ p: 0.25 }}
                        />
                      }
                      label={<Typography variant="caption">B{band}</Typography>}
                      sx={{ mr: 0.5, ml: 0 }}
                    />
                  ))}
                </Box>
              </Grid>

              {/* LTE TDD 频段 */}
              <Grid size={{ xs: 6, sm: 3 }}>
                <Typography variant="caption" color="text.secondary" gutterBottom display="block">LTE TDD (允许)</Typography>
                <Box sx={{ display: 'flex', flexWrap: 'wrap', gap: 0 }}>
                  {LTE_TDD_BANDS.map((band) => (
                    <FormControlLabel
                      key={`lte-tdd-${band}`}
                      control={
                        <Checkbox
                          checked={lteTddBands.includes(band)}
                          onChange={() => toggleBand(band, setLteTddBands)}
                          size="small"
                          sx={{ p: 0.25 }}
                        />
                      }
                      label={<Typography variant="caption">B{band}</Typography>}
                      sx={{ mr: 0.5, ml: 0 }}
                    />
                  ))}
                </Box>
              </Grid>

              {/* NR FDD 频段 */}
              <Grid size={{ xs: 6, sm: 3 }}>
                <Typography variant="caption" color="text.secondary" gutterBottom display="block">NR FDD (允许)</Typography>
                <Box sx={{ display: 'flex', flexWrap: 'wrap', gap: 0 }}>
                  {NR_FDD_BANDS.map((band) => (
                    <FormControlLabel
                      key={`nr-fdd-${band}`}
                      control={
                        <Checkbox
                          checked={nrFddBands.includes(band)}
                          onChange={() => toggleBand(band, setNrFddBands)}
                          size="small"
                          sx={{ p: 0.25 }}
                        />
                      }
                      label={<Typography variant="caption">n{band}</Typography>}
                      sx={{ mr: 0.5, ml: 0 }}
                    />
                  ))}
                </Box>
              </Grid>

              {/* NR TDD 频段 */}
              <Grid size={{ xs: 6, sm: 3 }}>
                <Typography variant="caption" color="text.secondary" gutterBottom display="block">NR TDD (允许)</Typography>
                <Box sx={{ display: 'flex', flexWrap: 'wrap', gap: 0 }}>
                  {NR_TDD_BANDS.map((band) => (
                    <FormControlLabel
                      key={`nr-tdd-${band}`}
                      control={
                        <Checkbox
                          checked={nrTddBands.includes(band)}
                          onChange={() => toggleBand(band, setNrTddBands)}
                          size="small"
                          sx={{ p: 0.25 }}
                        />
                      }
                      label={<Typography variant="caption">n{band}</Typography>}
                      sx={{ mr: 0.5, ml: 0 }}
                    />
                  ))}
                </Box>
              </Grid>
            </Grid>
            )}

            {/* 未锁定模式提示 */}
            {lockMode === 'unlocked' && (
              <Alert severity="success" sx={{ mb: 2 }}>
                当前模式：<strong>未锁定</strong><br />
                设备将使用所有支持的频段（B1, B3, B5, B8, B39, B41, N1, N3, N28, N41, N77, N78, N79）
              </Alert>
            )}

            {/* 自定义锁定模式提示 */}
            {lockMode === 'custom' && (
              <Alert severity="info" sx={{ mt: 1.5, mb: 1.5 }}>
                <Typography variant="caption" display="block" gutterBottom>
                  💡 <strong>提示</strong>：
                </Typography>
                <Typography variant="caption" display="block">
                  • 勾选的频段表示允许使用
                </Typography>
                <Typography variant="caption" display="block">
                  • 5G 频段：用于 5G 网络连接
                </Typography>
                <Typography variant="caption" display="block">
                  • 4G 频段：用于 4G 网络连接，以及 5G 信号弱时的回退
                </Typography>
              </Alert>
            )}

            {/* 操作按钮 */}
            <Box sx={{ mt: 1.5, display: 'flex', gap: 1, flexWrap: 'wrap' }}>
              <Button
                variant="contained"
                color="primary"
                size="small"
                onClick={() => void handleApplyBandLock()}
                disabled={bandLoading}
                startIcon={bandLoading ? <CircularProgress size={14} /> : <Lock />}
              >
                应用
              </Button>
              <Button
                variant="outlined"
                color="success"
                size="small"
                onClick={() => void handleUnlockAllBands()}
                disabled={bandLoading}
                startIcon={<LockOpen />}
              >
                取消限制
              </Button>
            </Box>
          </CardContent>
        </Card>

        {/* 基站定位参数 */}
        <Card sx={{ mt: 2 }}>
          <Accordion>
            <AccordionSummary expandIcon={<ExpandMore />}>
              <Box display="flex" alignItems="center" gap={1}>
                <MyLocation color="primary" />
                <Typography variant="subtitle1" fontWeight={600}>基站定位参数</Typography>
              </Box>
            </AccordionSummary>
            <AccordionDetails>
              {(() => {
                const cells = getAllLocationCells()
                return cells.length > 0 ? (
                  <>
                    <Alert severity="info" sx={{ mb: 2 }} icon={false}>
                      以下参数可用于第三方基站定位 API（高德、百度、Google）
                    </Alert>
                    <TableContainer component={Paper} variant="outlined">
                      <Table size="small">
                        <TableHead>
                          <TableRow>
                            <TableCell sx={{ py: 0.5, fontSize: '0.75rem' }}>MCC</TableCell>
                            <TableCell sx={{ py: 0.5, fontSize: '0.75rem' }}>MNC</TableCell>
                            <TableCell sx={{ py: 0.5, fontSize: '0.75rem' }}>LAC/TAC</TableCell>
                            <TableCell sx={{ py: 0.5, fontSize: '0.75rem' }}>CID</TableCell>
                            <TableCell sx={{ py: 0.5, fontSize: '0.75rem' }}>信号</TableCell>
                          </TableRow>
                        </TableHead>
                        <TableBody>
                          {cells.map((cell, idx) => (
                            <TableRow key={idx}>
                              <TableCell sx={{ py: 0.5, fontSize: '0.75rem' }}>{cell.mcc}</TableCell>
                              <TableCell sx={{ py: 0.5, fontSize: '0.75rem' }}>{cell.mnc}</TableCell>
                              <TableCell sx={{ py: 0.5, fontSize: '0.75rem' }}>{cell.lac}</TableCell>
                              <TableCell sx={{ py: 0.5, fontSize: '0.75rem', fontFamily: 'monospace' }}>{cell.cid}</TableCell>
                              <TableCell sx={{ py: 0.5, fontSize: '0.75rem' }}>{cell.signal_strength} dBm</TableCell>
                            </TableRow>
                          ))}
                        </TableBody>
                      </Table>
                    </TableContainer>
                    <Button
                      variant="outlined"
                      size="small"
                      startIcon={<ContentCopy />}
                      onClick={handleCopyCellLocation}
                      sx={{ mt: 1 }}
                    >
                      复制 JSON
                    </Button>
                  </>
                ) : (
                  <Alert severity="warning" icon={false}>暂无基站定位数据</Alert>
                )
              })()}
            </AccordionDetails>
          </Accordion>
        </Card>
      </TabPanel>

      {/* Tab 3: 网络接口 */}
      <TabPanel value={tabValue} index={2}>
        <Box display="flex" justifyContent="space-between" alignItems="center" mb={2}>
          <Box display="flex" alignItems="center" gap={2}>
            <FormControlLabel
              control={
                <Switch
                  checked={showIpAddresses}
                  onChange={(e: ChangeEvent<HTMLInputElement>) => setShowIpAddresses(e.target.checked)}
                  size="small"
                />
              }
              label={<Typography variant="body2" color="text.secondary">显示 IP 地址</Typography>}
            />
            <FormControlLabel
              control={
                <Switch
                  checked={showDownInterfaces}
                  onChange={(e: ChangeEvent<HTMLInputElement>) => setShowDownInterfaces(e.target.checked)}
                  size="small"
                />
              }
              label={<Typography variant="body2" color="text.secondary">显示已关闭接口</Typography>}
            />
          </Box>
          <Chip icon={<Router />} label={`${filteredInterfaces.length} / ${interfaces.length}`} color="primary" />
        </Box>

        <Grid container spacing={2}>
          {filteredInterfaces.map((iface) => (
            <Grid key={iface.name} size={12}>
              <Card>
                <CardHeader
                  avatar={<NetworkCheck color="primary" />}
                  title={
                    <Box display="flex" alignItems="center" gap={1}>
                      <Typography variant="h6">{iface.name}</Typography>
                      <Chip
                        label={iface.status.toUpperCase()}
                        size="small"
                        color={getInterfaceStatusColor(iface.status)}
                      />
                    </Box>
                  }
                  subheader={
                    <Box display="flex" gap={2} mt={0.5}>
                      {iface.mac_address && (
                        <Typography variant="caption" color="text.secondary">
                          MAC: {iface.mac_address}
                        </Typography>
                      )}
                      <Typography variant="caption" color="text.secondary">
                        MTU: {iface.mtu}
                      </Typography>
                    </Box>
                  }
                />
                <CardContent>
                  <Grid container spacing={2}>
                    {/* IP地址列表 */}
                    <Grid size={{ xs: 12, md: 6 }}>
                      <Typography variant="subtitle2" gutterBottom sx={{ display: 'flex', alignItems: 'center', gap: 1 }}>
                        <SignalCellularAlt fontSize="small" />
                        IP地址
                      </Typography>
                      <Divider sx={{ mb: 1 }} />
                      {iface.ip_addresses.length > 0 ? (
                        <Stack spacing={1}>
                          {iface.ip_addresses.map((ip: IpAddress, idx: number) => (
                            <Box
                              key={idx}
                              sx={{
                                p: 1,
                                border: '1px solid',
                                borderColor: 'divider',
                                borderRadius: 1,
                              }}
                            >
                              <Box display="flex" alignItems="center" gap={0.5} mb={0.5}>
                                <Chip
                                  icon={getScopeIcon(ip.scope)}
                                  label={getScopeLabel(ip.scope)}
                                  size="small"
                                  color={getScopeColor(ip.scope)}
                                />
                                <Chip label={ip.ip_type.toUpperCase()} size="small" variant="outlined" />
                              </Box>
                              <Typography variant="body2" sx={{ fontFamily: 'monospace', ...getIpAddressStyle() }}>
                                {ip.address}/{ip.prefix_len}
                              </Typography>
                            </Box>
                          ))}
                        </Stack>
                      ) : (
                        <Typography variant="body2" color="text.secondary">无IP地址</Typography>
                      )}
                    </Grid>

                    {/* 流量统计 */}
                    <Grid size={{ xs: 12, md: 6 }}>
                      <Typography variant="subtitle2" gutterBottom sx={{ display: 'flex', alignItems: 'center', gap: 1 }}>
                        <SignalCellularAlt fontSize="small" />
                        流量统计
                      </Typography>
                      <Divider sx={{ mb: 1 }} />
                      <TableContainer component={Paper} variant="outlined">
                        <Table size="small">
                          <TableHead>
                            <TableRow>
                              <TableCell>方向</TableCell>
                              <TableCell align="right">字节数</TableCell>
                              <TableCell align="right">包数</TableCell>
                              <TableCell align="right">错误</TableCell>
                            </TableRow>
                          </TableHead>
                          <TableBody>
                            <TableRow>
                              <TableCell><Chip label="RX" size="small" color="info" /></TableCell>
                              <TableCell align="right" sx={{ fontFamily: 'monospace' }}>{formatBytes(iface.rx_bytes)}</TableCell>
                              <TableCell align="right">{iface.rx_packets.toLocaleString()}</TableCell>
                              <TableCell align="right">
                                <Chip label={iface.rx_errors} size="small" color={iface.rx_errors > 0 ? 'error' : 'default'} />
                              </TableCell>
                            </TableRow>
                            <TableRow>
                              <TableCell><Chip label="TX" size="small" color="warning" /></TableCell>
                              <TableCell align="right" sx={{ fontFamily: 'monospace' }}>{formatBytes(iface.tx_bytes)}</TableCell>
                              <TableCell align="right">{iface.tx_packets.toLocaleString()}</TableCell>
                              <TableCell align="right">
                                <Chip label={iface.tx_errors} size="small" color={iface.tx_errors > 0 ? 'error' : 'default'} />
                              </TableCell>
                            </TableRow>
                          </TableBody>
                        </Table>
                      </TableContainer>
                    </Grid>
                  </Grid>
                </CardContent>
              </Card>
            </Grid>
          ))}
        </Grid>

        {filteredInterfaces.length === 0 && interfaces.length > 0 && (
          <Card>
            <CardContent>
              <Box textAlign="center" py={4}>
                <Typography variant="body1" color="text.secondary">
                  所有接口都处于关闭状态，打开"显示已关闭接口"开关以查看
                </Typography>
              </Box>
            </CardContent>
          </Card>
        )}
      </TabPanel>

      {/* Tab 2: APN 配置 */}
      <TabPanel value={tabValue} index={1}>
        <Grid container spacing={3}>
          <Grid size={{ xs: 12, md: 8 }}>
            <Card>
              <CardHeader
                avatar={<SimCard color="primary" />}
                title="APN 配置"
                titleTypographyProps={{ variant: 'h6' }}
                subheader="配置移动数据连接的接入点名称"
              />
              <CardContent>
                {apnContexts.length === 0 ? (
                  <Alert severity="warning">未找到可用的 APN 配置</Alert>
                ) : (
                  <Box display="flex" flexDirection="column" gap={2.5}>
                    {/* Context 选择 */}
                    <FormControl fullWidth>
                      <InputLabel>选择 APN 配置槽位</InputLabel>
                      <Select
                        value={selectedContext}
                        label="选择 APN 配置槽位"
                        onChange={(e) => handleContextChange(e.target.value)}
                      >
                        {apnContexts.map((ctx) => (
                          <MenuItem key={ctx.path} value={ctx.path}>
                            <Box display="flex" alignItems="center" gap={1} width="100%">
                              <Typography>{ctx.name} ({ctx.path.split('/').pop()})</Typography>
                              {ctx.active && <Chip label="已激活" size="small" color="success" />}
                              {ctx.apn && <Chip label={ctx.apn} size="small" variant="outlined" />}
                            </Box>
                          </MenuItem>
                        ))}
                      </Select>
                    </FormControl>

                    <Divider />

                    {/* APN 名称 */}
                    <TextField
                      label="APN 名称"
                      value={apnForm.apn}
                      onChange={(e: ChangeEvent<HTMLInputElement>) => setApnForm({ ...apnForm, apn: e.target.value })}
                      fullWidth
                      placeholder="例如: cbnet, cmnet, 3gnet"
                      helperText="运营商提供的接入点名称"
                    />

                    {/* 协议选择 */}
                    <FormControl fullWidth>
                      <InputLabel>IP 协议</InputLabel>
                      <Select
                        value={apnForm.protocol}
                        label="IP 协议"
                        onChange={(e) => setApnForm({ ...apnForm, protocol: e.target.value })}
                      >
                        <MenuItem value="ip">IPv4</MenuItem>
                        <MenuItem value="ipv6">IPv6</MenuItem>
                        <MenuItem value="dual">IPv4v6 (双栈，推荐)</MenuItem>
                      </Select>
                    </FormControl>

                    {/* 用户名和密码 */}
                    <Grid container spacing={2}>
                      <Grid size={{ xs: 12, sm: 6 }}>
                        <TextField
                          label="用户名"
                          value={apnForm.username}
                          onChange={(e: ChangeEvent<HTMLInputElement>) => setApnForm({ ...apnForm, username: e.target.value })}
                          fullWidth
                          placeholder="可选"
                        />
                      </Grid>
                      <Grid size={{ xs: 12, sm: 6 }}>
                        <TextField
                          label="密码"
                          type="password"
                          value={apnForm.password}
                          onChange={(e: ChangeEvent<HTMLInputElement>) => setApnForm({ ...apnForm, password: e.target.value })}
                          fullWidth
                          placeholder="可选"
                        />
                      </Grid>
                    </Grid>

                    {/* 认证方式 */}
                    <FormControl fullWidth>
                      <InputLabel>认证方式</InputLabel>
                      <Select
                        value={apnForm.auth_method}
                        label="认证方式"
                        onChange={(e) => setApnForm({ ...apnForm, auth_method: e.target.value })}
                      >
                        <MenuItem value="none">无</MenuItem>
                        <MenuItem value="pap">PAP</MenuItem>
                        <MenuItem value="chap">CHAP (推荐)</MenuItem>
                      </Select>
                    </FormControl>

                    {/* 保存按钮 */}
                    <Button
                      variant="contained"
                      color="primary"
                      size="large"
                      onClick={() => void saveApn()}
                      disabled={apnSaving || !selectedContext || !apnForm.apn}
                      startIcon={apnSaving ? <CircularProgress size={20} /> : undefined}
                    >
                      {apnSaving ? '保存中...' : '保存 APN 配置'}
                    </Button>
                  </Box>
                )}
              </CardContent>
            </Card>
          </Grid>

          {/* 右侧信息面板 */}
          <Grid size={{ xs: 12, md: 4 }}>
            {/* 当前状态 */}
            {selectedContext && (
              <Card sx={{ mb: 2 }}>
                <CardHeader title="当前配置状态" titleTypographyProps={{ variant: 'subtitle1' }} />
                <CardContent>
                  <Stack spacing={1}>
                    <Chip 
                      label={apnContexts.find(c => c.path === selectedContext)?.active ? '已激活' : '未激活'}
                      color={apnContexts.find(c => c.path === selectedContext)?.active ? 'success' : 'default'}
                      sx={{ justifyContent: 'flex-start' }}
                    />
                    <Chip 
                      label={`协议: ${getProtocolName(apnContexts.find(c => c.path === selectedContext)?.protocol || 'ip')}`}
                      variant="outlined"
                      sx={{ justifyContent: 'flex-start' }}
                    />
                    {apnContexts.find(c => c.path === selectedContext)?.apn && (
                      <Chip 
                        label={`APN: ${apnContexts.find(c => c.path === selectedContext)?.apn}`}
                        color="primary"
                        variant="outlined"
                        sx={{ justifyContent: 'flex-start' }}
                      />
                    )}
                  </Stack>
                </CardContent>
              </Card>
            )}

            {/* 常用 APN 参考 */}
            <Card>
              <CardHeader title="常用运营商 APN" titleTypographyProps={{ variant: 'subtitle1' }} />
              <CardContent>
                <Table size="small">
                  <TableBody>
                    <TableRow>
                      <TableCell><strong>中国移动</strong></TableCell>
                      <TableCell>cmnet</TableCell>
                    </TableRow>
                    <TableRow>
                      <TableCell><strong>中国联通</strong></TableCell>
                      <TableCell>3gnet / 3gwap</TableCell>
                    </TableRow>
                    <TableRow>
                      <TableCell><strong>中国电信</strong></TableCell>
                      <TableCell>ctnet / ctlte</TableCell>
                    </TableRow>
                    <TableRow>
                      <TableCell><strong>中国广电</strong></TableCell>
                      <TableCell>cbnet</TableCell>
                    </TableRow>
                  </TableBody>
                </Table>
              </CardContent>
            </Card>
          </Grid>
        </Grid>
      </TabPanel>
    </Box>
  );
}
