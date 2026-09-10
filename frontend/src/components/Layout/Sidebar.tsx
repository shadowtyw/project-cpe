/*
 * @Author: 1orz cloudorzi@gmail.com
 * @Date: 2025-12-10 09:19:05
 * @LastEditors: 1orz cloudorzi@gmail.com
 * @LastEditTime: 2026-04-18 00:00:00
 * @FilePath: /udx710-backend/frontend/src/components/Layout/Sidebar.tsx
 * @Description:
 *
 * Copyright (c) 2025 by 1orz, All Rights Reserved.
 */
import { useState, useEffect, useMemo } from 'react'
import { useNavigate, useLocation } from 'react-router-dom'
import {
  Drawer,
  List,
  ListItem,
  ListItemButton,
  ListItemIcon,
  ListItemText,
  Toolbar,
  Divider,
  Box,
  Typography,
  Link,
  Collapse,
} from '@mui/material'
import {
  Dashboard as DashboardIcon,
  Devices as DevicesIcon,
  SignalCellularAlt as SignalIcon,
  Settings as SettingsIcon,
  Terminal as TerminalIcon,
  Phone as PhoneIcon,
  Sms as SmsIcon,
  GitHub as GitHubIcon,
  WebAsset as WebTerminalIcon,
  Memory as MemoryIcon,
  SystemUpdateAlt as OtaIcon,
  RocketLaunch as InitScriptIcon,
  Article as LogsIcon,
  Build as ToolsIcon,
  PhonelinkSetup as PhonelinkSetupIcon,
  ExpandMore as ExpandMoreIcon,
  ExpandLess as ExpandLessIcon,
} from '@mui/icons-material'

interface SidebarProps {
  drawerWidth: number
  mobileOpen: boolean
  desktopOpen: boolean
  onClose: () => void
  isMobile: boolean
}

interface MenuItem {
  path?: string
  label: string
  icon: React.ComponentType
  children?: MenuItem[]
}

const menuItems: MenuItem[] = [
  { path: '/', label: '仪表盘', icon: DashboardIcon },
  { path: '/device', label: '设备信息', icon: DevicesIcon },
  { path: '/network', label: '网络状态', icon: SignalIcon },
  { path: '/phone', label: '电话管理', icon: PhoneIcon },
  { path: '/sms', label: '短信管理', icon: SmsIcon },
  { path: '/config', label: '系统配置', icon: SettingsIcon },
  { path: '/init-script', label: '开机脚本', icon: InitScriptIcon },
  { path: '/ota', label: 'OTA 更新', icon: OtaIcon },
  {
    label: '远程遥控',
    icon: PhonelinkSetupIcon,
    children: [
      { path: '/remote/sms', label: '短信遥控', icon: SmsIcon },
      { path: '/remote/call', label: '通话遥控', icon: PhoneIcon },
    ],
  },
  { path: '/memory-processes', label: '内存进程', icon: MemoryIcon },
  { path: '/logs', label: '系统日志', icon: LogsIcon },
  { path: '/tools', label: '高级工具', icon: ToolsIcon },
  { path: '/at-console', label: 'AT 控制台', icon: TerminalIcon },
  { path: '/terminal', label: 'Web 终端', icon: WebTerminalIcon },
]

export default function Sidebar({ drawerWidth, mobileOpen, desktopOpen, onClose, isMobile }: SidebarProps) {
  const navigate = useNavigate()
  const location = useLocation()

  // 展开状态持久化到 localStorage
  const [expandedMenus, setExpandedMenus] = useState<Set<string>>(() => {
    try {
      const saved = localStorage.getItem('expandedMenus')
      return saved ? new Set(JSON.parse(saved) as string[]) : new Set<string>()
    } catch {
      return new Set<string>()
    }
  })

  // 自动展开包含当前路径的父菜单（在 render 期间计算，不在 effect 中）
  const effectiveExpandedMenus = useMemo(() => {
    const next = new Set(expandedMenus)
    for (const item of menuItems) {
      if (item.children) {
        const hasMatch = item.children.some((child) => child.path === location.pathname)
        if (hasMatch) {
          next.add(item.label)
        }
      }
    }
    return next
  }, [expandedMenus, location.pathname])

  useEffect(() => {
    localStorage.setItem('expandedMenus', JSON.stringify([...effectiveExpandedMenus]))
  }, [effectiveExpandedMenus])

  const handleNavigation = (path: string): void => {
    void navigate(path)
    if (isMobile) {
      onClose()
    }
  }

  const toggleMenu = (label: string) => {
    setExpandedMenus((prev) => {
      const next = new Set(prev)
      if (next.has(label)) {
        next.delete(label)
      } else {
        next.add(label)
      }
      return next
    })
  }

  const isChildActive = (children?: MenuItem[]) => {
    if (!children) return false
    return children.some((child) => child.path === location.pathname)
  }

  const drawer = (
    <Box sx={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      <Toolbar>
        <Box sx={{ display: 'flex', alignItems: 'center', gap: 1 }}>
          <Typography variant="h6" noWrap component="div" fontWeight={600}>
            UDX710
          </Typography>
        </Box>
      </Toolbar>
      <Divider />
      <List sx={{ flexGrow: 1 }}>
        {menuItems.map((item) => {
          const IconComponent = item.icon

          // 有子菜单的父级项
          if (item.children) {
            const isExpanded = effectiveExpandedMenus.has(item.label)
            const childActive = isChildActive(item.children)

            return (
              <Box key={item.label}>
                <ListItem disablePadding>
                  <ListItemButton
                    selected={childActive}
                    onClick={() => toggleMenu(item.label)}
                  >
                    <ListItemIcon>
                      <IconComponent />
                    </ListItemIcon>
                    <ListItemText primary={item.label} />
                    {isExpanded ? <ExpandLessIcon /> : <ExpandMoreIcon />}
                  </ListItemButton>
                </ListItem>
                <Collapse in={isExpanded} timeout="auto" unmountOnExit>
                  <List component="div" disablePadding>
                    {item.children.map((child) => {
                      const ChildIcon = child.icon
                      return (
                        <ListItem key={child.path} disablePadding>
                          <ListItemButton
                            selected={location.pathname === child.path}
                            onClick={() => child.path && handleNavigation(child.path)}
                            sx={{ pl: 4 }}
                          >
                            <ListItemIcon>
                              <ChildIcon />
                            </ListItemIcon>
                            <ListItemText primary={child.label} />
                          </ListItemButton>
                        </ListItem>
                      )
                    })}
                  </List>
                </Collapse>
              </Box>
            )
          }

          // 普通菜单项
          return (
            <ListItem key={item.path} disablePadding>
              <ListItemButton
                selected={location.pathname === item.path}
                onClick={() => item.path && handleNavigation(item.path)}
              >
                <ListItemIcon>
                  <IconComponent />
                </ListItemIcon>
                <ListItemText primary={item.label} />
              </ListItemButton>
            </ListItem>
          )
        })}
      </List>
      <Box sx={{ p: 2, borderTop: 1, borderColor: 'divider' }}>
        <Link
          href="https://github.com/1orz/project-cpe"
          target="_blank"
          rel="noopener noreferrer"
          sx={{
            display: 'flex',
            alignItems: 'center',
            gap: 0.5,
            color: 'text.secondary',
            textDecoration: 'none',
            fontSize: '0.75rem',
            '&:hover': {
              color: 'primary.main',
            },
          }}
        >
          <GitHubIcon sx={{ fontSize: 16 }} />
          <Typography variant="caption" color="inherit">
            1orz/project-cpe
          </Typography>
        </Link>
        <Typography variant="caption" color="text.disabled" sx={{ display: 'block', mt: 0.5 }}>
          v{__APP_VERSION__} ({__GIT_BRANCH__}/{__GIT_COMMIT__})
        </Typography>
        <Typography variant="caption" color="text.disabled" sx={{ display: 'block', mt: 0.5 }}>
          Copyright 2025 1orz
        </Typography>
      </Box>
    </Box>
  )

  return (
    <Box
      component="nav"
      sx={{
        width: { xs: 0, sm: desktopOpen ? drawerWidth : 0 },
        flexShrink: { sm: 0 },
        transition: 'width 0.3s',
      }}
    >
      <Drawer
        variant="temporary"
        open={mobileOpen}
        onClose={onClose}
        ModalProps={{
          keepMounted: true,
        }}
        sx={{
          display: { xs: 'block', sm: 'none' },
          '& .MuiDrawer-paper': {
            boxSizing: 'border-box',
            width: drawerWidth,
          },
        }}
      >
        {drawer}
      </Drawer>

      <Drawer
        variant="persistent"
        open={desktopOpen}
        sx={{
          display: { xs: 'none', sm: 'block' },
          '& .MuiDrawer-paper': {
            boxSizing: 'border-box',
            width: drawerWidth,
            transition: 'transform 0.3s',
          },
        }}
      >
        {drawer}
      </Drawer>
    </Box>
  )
}
