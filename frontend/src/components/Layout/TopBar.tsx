/*
 * @Author: 1orz cloudorzi@gmail.com
 * @Date: 2025-11-22 10:30:41
 * @LastEditors: 1orz cloudorzi@gmail.com
 * @LastEditTime: 2025-12-13 12:43:22
 * @FilePath: /udx710-backend/frontend/src/components/Layout/TopBar.tsx
 * @Description:
 *
 * Copyright (c) 2025 by 1orz, All Rights Reserved.
 */
import { useState } from 'react'
import {
  AppBar,
  Toolbar,
  Typography,
  IconButton,
  Box,
  Menu,
  MenuItem,
  ListItemIcon,
  ListItemText,
  Divider,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogContentText,
  DialogTitle,
  CircularProgress,
  Alert,
} from '@mui/material'
import {
  Menu as MenuIcon,
  Refresh as RefreshIcon,
  MoreVert as MoreVertIcon,
  Brightness4 as DarkModeIcon,
  Brightness7 as LightModeIcon,
  Speed as SpeedIcon,
  RestartAlt as RestartAltIcon,
} from '@mui/icons-material'
import { api } from '../../api'
import { useTheme } from '../../contexts/ThemeContext'
import { useRefreshInterval } from '../../contexts/RefreshContext'

interface TopBarProps {
  drawerWidth: number
  onMenuClick: () => void
  refreshInterval: number
  onRefreshIntervalChange: (interval: number) => void
}

export default function TopBar({
  drawerWidth,
  onMenuClick,
  refreshInterval,
  onRefreshIntervalChange,
}: TopBarProps) {
  const { mode, toggleTheme } = useTheme()
  const { triggerRefresh } = useRefreshInterval()
  const [anchorEl, setAnchorEl] = useState<null | HTMLElement>(null)
  const [refreshMenuAnchor, setRefreshMenuAnchor] = useState<null | HTMLElement>(null)
  const [rebootDialogOpen, setRebootDialogOpen] = useState(false)
  const [rebooting, setRebooting] = useState(false)
  const [rebootError, setRebootError] = useState<string | null>(null)

  const handleMenuOpen = (event: React.MouseEvent<HTMLElement>) => {
    setAnchorEl(event.currentTarget)
  }

  const handleMenuClose = () => {
    setAnchorEl(null)
  }

  const handleRefreshMenuOpen = (event: React.MouseEvent<HTMLElement>) => {
    setRefreshMenuAnchor(event.currentTarget)
  }

  const handleRefreshMenuClose = () => {
    setRefreshMenuAnchor(null)
  }

  const handleRefreshIntervalChange = (interval: number) => {
    onRefreshIntervalChange(interval)
    handleRefreshMenuClose()
  }

  const handleRefresh = () => {
    triggerRefresh()
  }

  const handleThemeToggle = () => {
    toggleTheme()
    handleMenuClose()
  }

  const handleReboot = async () => {
    setRebooting(true)
    setRebootError(null)
    try {
      await api.systemReboot(3)
      setRebootDialogOpen(false)
    } catch (error) {
      setRebootError(error instanceof Error ? error.message : String(error))
    } finally {
      setRebooting(false)
    }
  }

  const getRefreshLabel = () => {
    if (refreshInterval === 0) return '手动'
    if (refreshInterval === 1000) return '1秒'
    if (refreshInterval === 3000) return '3秒'
    if (refreshInterval === 5000) return '5秒'
    if (refreshInterval === 10000) return '10秒'
    return `${refreshInterval / 1000}秒`
  }

  return (
    <AppBar
      position="fixed"
      sx={{
        width: { sm: `calc(100% - ${drawerWidth}px)` },
        ml: { sm: `${drawerWidth}px` },
      }}
    >
      <Toolbar sx={{ minHeight: { xs: 56, sm: 64 } }}>
        <IconButton
          color="inherit"
          aria-label="切换侧边栏"
          edge="start"
          onClick={onMenuClick}
          sx={{ mr: 2 }}
        >
          <MenuIcon />
        </IconButton>

        <Typography
          variant="h6"
          noWrap
          component="div"
          sx={{
            flexGrow: 1,
            fontSize: { xs: '1rem', sm: '1.25rem' },
          }}
        >
          控制面板
        </Typography>

        <Box sx={{ display: 'flex', alignItems: 'center', gap: { xs: 0.5, sm: 1 } }}>
          <Button
            color="inherit"
            size="small"
            startIcon={<RestartAltIcon />}
            onClick={() => setRebootDialogOpen(true)}
            disabled={rebooting}
            sx={{ display: { xs: 'none', sm: 'inline-flex' } }}
          >
            重启设备
          </Button>
          <IconButton
            color="inherit"
            aria-label="重启设备"
            onClick={() => setRebootDialogOpen(true)}
            disabled={rebooting}
            title="重启设备"
            sx={{ display: { xs: 'inline-flex', sm: 'none' } }}
          >
            <RestartAltIcon />
          </IconButton>
          <IconButton
            color="inherit"
            onClick={handleRefresh}
            title="刷新页面"
            sx={{ display: { xs: 'inline-flex', sm: 'inline-flex' } }}
          >
            <RefreshIcon />
          </IconButton>
          <IconButton
            color="inherit"
            onClick={handleMenuOpen}
            title="更多选项"
            sx={{ display: { xs: 'inline-flex', sm: 'inline-flex' } }}
          >
            <MoreVertIcon />
          </IconButton>
        </Box>

        <Menu
          anchorEl={anchorEl}
          open={Boolean(anchorEl)}
          onClose={handleMenuClose}
          anchorOrigin={{ vertical: 'bottom', horizontal: 'right' }}
          transformOrigin={{ vertical: 'top', horizontal: 'right' }}
          PaperProps={{ sx: { minWidth: 200, mt: 1 } }}
        >
          <MenuItem onClick={handleThemeToggle}>
            <ListItemIcon>
              {mode === 'dark' ? <LightModeIcon fontSize="small" /> : <DarkModeIcon fontSize="small" />}
            </ListItemIcon>
            <ListItemText>{mode === 'dark' ? '浅色模式' : '深色模式'}</ListItemText>
          </MenuItem>
          <Divider />
          <MenuItem onClick={handleRefreshMenuOpen}>
            <ListItemIcon><SpeedIcon fontSize="small" /></ListItemIcon>
            <ListItemText primary="刷新频率" secondary={getRefreshLabel()} secondaryTypographyProps={{ variant: 'caption' }} />
          </MenuItem>
        </Menu>

        <Menu
          anchorEl={refreshMenuAnchor}
          open={Boolean(refreshMenuAnchor)}
          onClose={handleRefreshMenuClose}
          anchorOrigin={{ vertical: 'top', horizontal: 'left' }}
          transformOrigin={{ vertical: 'top', horizontal: 'right' }}
          PaperProps={{ sx: { minWidth: 150 } }}
        >
          {[1000, 3000, 5000, 10000].map((interval) => (
            <MenuItem key={interval} selected={refreshInterval === interval} onClick={() => handleRefreshIntervalChange(interval)}>
              {interval / 1000}秒/次
            </MenuItem>
          ))}
          <Divider />
          <MenuItem selected={refreshInterval === 0} onClick={() => handleRefreshIntervalChange(0)}>手动刷新</MenuItem>
        </Menu>

        <Dialog open={rebootDialogOpen} onClose={() => !rebooting && setRebootDialogOpen(false)}>
          <DialogTitle>确认重启设备</DialogTitle>
          <DialogContent>
            {rebootError && <Alert severity="error" sx={{ mb: 2 }}>{rebootError}</Alert>}
            <DialogContentText>
              设备将在约 3 秒后重启。当前蜂窝数据、USB 网络、通话和后台管理连接都会暂时中断。
            </DialogContentText>
          </DialogContent>
          <DialogActions>
            <Button onClick={() => setRebootDialogOpen(false)} disabled={rebooting}>取消</Button>
            <Button onClick={() => void handleReboot()} variant="contained" color="warning" startIcon={rebooting ? <CircularProgress size={18} /> : <RestartAltIcon />} disabled={rebooting}>
              {rebooting ? '正在安排…' : '确认重启'}
            </Button>
          </DialogActions>
        </Dialog>
      </Toolbar>
    </AppBar>
  )
}
