import { useCallback, useEffect, useRef, useState } from 'react'
import {
  Alert,
  AlertTitle,
  Box,
  Button,
  Card,
  CardContent,
  Checkbox,
  Chip,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogContentText,
  DialogTitle,
  Divider,
  FormControlLabel,
  LinearProgress,
  Paper,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableRow,
  Typography,
} from '@mui/material'
import {
  Cancel,
  CheckCircle,
  CloudUpload,
  Error as ErrorIcon,
  Info,
  Refresh,
  RestartAlt,
  Restore,
  SystemUpdateAlt,
  Warning,
} from '@mui/icons-material'
import { api } from '../api'
import type { OtaStatusResponse, OtaUploadResponse } from '../api/types'

type ConfirmDialog = 'apply' | 'downgrade' | 'rollback' | 'cancel' | null

function isRecoveryCandidate(status: OtaStatusResponse | null) {
  return status?.pending_validation?.valid
    && status.pending_validation.version_relation !== 'upgrade'
}

export default function OtaUpdate() {
  const [loading, setLoading] = useState(true)
  const [uploading, setUploading] = useState(false)
  const [applying, setApplying] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)
  const [status, setStatus] = useState<OtaStatusResponse | null>(null)
  const [uploadResult, setUploadResult] = useState<OtaUploadResponse | null>(null)
  const [confirmDialog, setConfirmDialog] = useState<ConfirmDialog>(null)
  const [downgradeAcknowledged, setDowngradeAcknowledged] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)

  const loadStatus = useCallback(async () => {
    try {
      const response = await api.getOtaStatus()
      if (response.data) {
        setStatus(response.data)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadStatus()
  }, [loadStatus])

  const handleFileSelect = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]
    if (!file) return

    if (!['.tar.gz', '.tgz'].some((extension) => file.name.endsWith(extension))) {
      setError('请上传 GitHub Release 或构建脚本生成的 .tar.gz OTA 更新包')
      return
    }

    setUploading(true)
    setError(null)
    setSuccess(null)
    setUploadResult(null)

    try {
      const response = await api.uploadOta(file)
      if (response.data) {
        setUploadResult(response.data)
        const relation = response.data.validation.version_relation
        if (response.data.validation.valid && relation === 'upgrade') {
          setSuccess('OTA 包上传成功，可直接应用更新')
        } else if (response.data.validation.valid) {
          setSuccess('OTA 包上传成功。这是恢复候选包，应用前必须明确确认降级。')
        } else {
          setError(`OTA 包验证失败：${response.data.validation.error || '未知错误'}`)
        }
        await loadStatus()
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setUploading(false)
      if (fileInputRef.current) {
        fileInputRef.current.value = ''
      }
    }
  }

  const handleApply = async (restartNow: boolean, allowDowngrade = false) => {
    setConfirmDialog(null)
    setApplying(true)
    setError(null)
    setSuccess(null)

    try {
      const response = await api.applyOta(restartNow, allowDowngrade)
      if (response.status === 'ok') {
        setSuccess(restartNow ? '更新已应用，设备即将重启…' : '更新已应用，请手动重启服务后确认版本')
        setUploadResult(null)
        setDowngradeAcknowledged(false)
        await loadStatus()
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setApplying(false)
    }
  }

  const handleRollback = async (restartNow: boolean) => {
    setConfirmDialog(null)
    setApplying(true)
    setError(null)
    setSuccess(null)

    try {
      const response = await api.rollbackOta(restartNow)
      if (response.status === 'ok') {
        setSuccess(restartNow ? '上一版本已恢复，设备即将重启…' : '上一版本已恢复，请手动重启服务后确认版本')
        await loadStatus()
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setApplying(false)
    }
  }

  const handleCancel = async () => {
    setConfirmDialog(null)
    setError(null)
    setSuccess(null)

    try {
      const response = await api.cancelOta()
      if (response.status === 'ok') {
        setSuccess('已取消待安装的更新')
        setUploadResult(null)
        await loadStatus()
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const pendingRecovery = isRecoveryCandidate(status)
  const pendingRelation = status?.pending_validation?.version_relation

  if (loading) {
    return (
      <Box display="flex" justifyContent="center" alignItems="center" minHeight="60vh">
        <CircularProgress />
      </Box>
    )
  }

  return (
    <Box>
      <Box display="flex" justifyContent="space-between" alignItems="center" mb={3}>
        <Box>
          <Typography variant="h4" gutterBottom fontWeight={600}>OTA 更新</Typography>
          <Typography variant="body2" color="text.secondary">上传、升级或恢复设备软件</Typography>
        </Box>
        <Button variant="outlined" startIcon={<Refresh />} onClick={() => void loadStatus()} disabled={applying}>
          刷新状态
        </Button>
      </Box>

      {error && <Alert severity="error" sx={{ mb: 2 }} onClose={() => setError(null)}>{error}</Alert>}
      {success && <Alert severity="success" sx={{ mb: 2 }} onClose={() => setSuccess(null)}>{success}</Alert>}

      <Stack spacing={3}>
        <Card>
          <CardContent>
            <Box display="flex" alignItems="center" gap={1} mb={2}>
              <Info color="primary" />
              <Typography variant="h6">当前版本</Typography>
            </Box>
            <TableContainer>
              <Table size="small"><TableBody>
                <TableRow><TableCell component="th" sx={{ width: 150 }}>版本号</TableCell><TableCell><Chip label={status?.current_version || 'N/A'} color="primary" size="small" /></TableCell></TableRow>
                <TableRow><TableCell component="th">Commit</TableCell><TableCell sx={{ fontFamily: 'monospace' }}>{status?.current_commit || 'N/A'}</TableCell></TableRow>
              </TableBody></Table>
            </TableContainer>
          </CardContent>
        </Card>

        {status?.rollback_available && status.rollback_meta && (
          <Card sx={{ borderColor: 'info.main', borderWidth: 2, borderStyle: 'solid' }}>
            <CardContent>
              <Box display="flex" alignItems="center" gap={1} mb={2}>
                <Restore color="info" />
                <Typography variant="h6">恢复上一版本</Typography>
                <Chip label={status.rollback_meta.version} color="info" size="small" />
              </Box>
              <Typography variant="body2" color="text.secondary" paragraph>
                此快照是上一次成功安装前自动保留的本机版本。恢复后，当前版本将成为下一次可恢复的版本。
              </Typography>
              <TableContainer><Table size="small"><TableBody>
                <TableRow><TableCell component="th" sx={{ width: 150 }}>可恢复版本</TableCell><TableCell>{status.rollback_meta.version}</TableCell></TableRow>
                <TableRow><TableCell component="th">Commit</TableCell><TableCell sx={{ fontFamily: 'monospace' }}>{status.rollback_meta.commit}</TableCell></TableRow>
              </TableBody></Table></TableContainer>
              <Divider sx={{ my: 2 }} />
              <Button variant="contained" color="warning" startIcon={<Restore />} onClick={() => setConfirmDialog('rollback')} disabled={applying}>
                恢复上一版本
              </Button>
            </CardContent>
          </Card>
        )}

        {status?.pending_update && status.pending_meta && (
          <Card sx={{ borderColor: pendingRecovery ? 'warning.main' : 'success.main', borderWidth: 2, borderStyle: 'solid' }}>
            <CardContent>
              <Box display="flex" alignItems="center" gap={1} mb={2}>
                <Warning color={pendingRecovery ? 'warning' : 'success'} />
                <Typography variant="h6">待安装更新</Typography>
                <Chip label={status.pending_meta.version} color={pendingRecovery ? 'warning' : 'success'} size="small" />
              </Box>
              {pendingRecovery ? (
                <Alert severity="warning" sx={{ mb: 2 }}>
                  该包相对当前版本是{pendingRelation === 'same' ? '同版本重装' : '降级恢复'}。完整性校验已通过，但必须明确确认后才会应用。
                </Alert>
              ) : (
                <Alert severity="success" sx={{ mb: 2 }}>这是较新版本，可按常规流程应用。</Alert>
              )}
              <TableContainer><Table size="small"><TableBody>
                <TableRow><TableCell component="th" sx={{ width: 150 }}>版本号</TableCell><TableCell>{status.pending_meta.version}</TableCell></TableRow>
                <TableRow><TableCell component="th">Commit</TableCell><TableCell sx={{ fontFamily: 'monospace' }}>{status.pending_meta.commit}</TableCell></TableRow>
                <TableRow><TableCell component="th">构建时间</TableCell><TableCell>{status.pending_meta.build_time}</TableCell></TableRow>
                <TableRow><TableCell component="th">架构</TableCell><TableCell>{status.pending_meta.arch}</TableCell></TableRow>
              </TableBody></Table></TableContainer>
              <Divider sx={{ my: 2 }} />
              <Stack direction="row" spacing={2} flexWrap="wrap">
                <Button
                  variant="contained"
                  color={pendingRecovery ? 'warning' : 'success'}
                  startIcon={<SystemUpdateAlt />}
                  onClick={() => setConfirmDialog(pendingRecovery ? 'downgrade' : 'apply')}
                  disabled={applying}
                >
                  {applying ? <CircularProgress size={20} /> : pendingRecovery ? '确认恢复此包' : '应用更新'}
                </Button>
                <Button variant="outlined" color="error" startIcon={<Cancel />} onClick={() => setConfirmDialog('cancel')} disabled={applying}>取消更新</Button>
              </Stack>
            </CardContent>
          </Card>
        )}

        <Card>
          <CardContent>
            <Box display="flex" alignItems="center" gap={1} mb={2}>
              <CloudUpload color="primary" />
              <Typography variant="h6">上传更新包</Typography>
            </Box>
            <Alert severity="info" sx={{ mb: 2 }}>
              <AlertTitle>OTA 包格式</AlertTitle>
              请上传 GitHub Release 或构建脚本生成的 <code>.tar.gz</code> 包。包必须包含 <code>meta.json</code>、<code>udx710</code> 和 <code>www/</code>。
            </Alert>
            <input ref={fileInputRef} type="file" accept=".tar.gz,.tgz,application/gzip,application/x-gzip,application/x-tar" style={{ display: 'none' }} onChange={(event) => void handleFileSelect(event)} />
            <Button variant="contained" startIcon={uploading ? <CircularProgress size={20} color="inherit" /> : <CloudUpload />} onClick={() => fileInputRef.current?.click()} disabled={uploading || applying} size="large">
              {uploading ? '上传中…' : '选择更新包'}
            </Button>
            {uploading && <Box sx={{ mt: 2 }}><LinearProgress /></Box>}
          </CardContent>
        </Card>

        {uploadResult && (
          <Card>
            <CardContent>
              <Box display="flex" alignItems="center" gap={1} mb={2}>
                {uploadResult.validation.valid ? <CheckCircle color="success" /> : <ErrorIcon color="error" />}
                <Typography variant="h6">验证结果</Typography>
                <Chip label={uploadResult.validation.valid ? '通过' : '失败'} color={uploadResult.validation.valid ? 'success' : 'error'} size="small" />
              </Box>
              <TableContainer component={Paper} variant="outlined"><Table size="small"><TableBody>
                <TableRow><TableCell component="th" sx={{ width: 180 }}>版本号</TableCell><TableCell>{uploadResult.meta.version}</TableCell><TableCell align="right"><Chip label={uploadResult.validation.version_relation === 'upgrade' ? '新版本' : uploadResult.validation.version_relation === 'same' ? '同版本恢复' : '降级恢复'} color={uploadResult.validation.version_relation === 'upgrade' ? 'success' : 'warning'} size="small" /></TableCell></TableRow>
                <TableRow><TableCell component="th">二进制 MD5</TableCell><TableCell sx={{ fontFamily: 'monospace', fontSize: '0.75rem' }}>{uploadResult.meta.binary_md5}</TableCell><TableCell align="right">{uploadResult.validation.binary_md5_match ? <CheckCircle color="success" fontSize="small" /> : <ErrorIcon color="error" fontSize="small" />}</TableCell></TableRow>
                <TableRow><TableCell component="th">前端 MD5</TableCell><TableCell sx={{ fontFamily: 'monospace', fontSize: '0.75rem' }}>{uploadResult.meta.frontend_md5}</TableCell><TableCell align="right">{uploadResult.validation.frontend_md5_match ? <CheckCircle color="success" fontSize="small" /> : <ErrorIcon color="error" fontSize="small" />}</TableCell></TableRow>
                <TableRow><TableCell component="th">架构</TableCell><TableCell>{uploadResult.meta.arch}</TableCell><TableCell align="right">{uploadResult.validation.arch_match ? <CheckCircle color="success" fontSize="small" /> : <ErrorIcon color="error" fontSize="small" />}</TableCell></TableRow>
              </TableBody></Table></TableContainer>
              {uploadResult.validation.error && <Alert severity="error" sx={{ mt: 2 }}>{uploadResult.validation.error}</Alert>}
            </CardContent>
          </Card>
        )}
      </Stack>

      <Dialog open={confirmDialog === 'apply'} onClose={() => setConfirmDialog(null)}>
        <DialogTitle>确认应用更新</DialogTitle>
        <DialogContent><DialogContentText>更新将替换当前后端程序和前端文件，并自动保存当前版本用于后续恢复。</DialogContentText></DialogContent>
        <DialogActions>
          <Button onClick={() => setConfirmDialog(null)}>取消</Button>
          <Button onClick={() => void handleApply(false)} variant="outlined">仅应用（稍后重启）</Button>
          <Button onClick={() => void handleApply(true)} variant="contained" color="success" startIcon={<RestartAlt />}>应用并重启</Button>
        </DialogActions>
      </Dialog>

      <Dialog open={confirmDialog === 'downgrade'} onClose={() => setConfirmDialog(null)}>
        <DialogTitle>确认降级恢复</DialogTitle>
        <DialogContent>
          <Alert severity="warning" sx={{ mb: 2 }}>你正在安装同版本或低版本包。该操作适合恢复已知可用版本，不适合日常更新。</Alert>
          <FormControlLabel control={<Checkbox checked={downgradeAcknowledged} onChange={(event) => setDowngradeAcknowledged(event.target.checked)} />} label="我理解这是降级/重装操作，并确认要继续" />
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setConfirmDialog(null)}>取消</Button>
          <Button onClick={() => void handleApply(false, true)} disabled={!downgradeAcknowledged} variant="outlined" color="warning">恢复（稍后重启）</Button>
          <Button onClick={() => void handleApply(true, true)} disabled={!downgradeAcknowledged} variant="contained" color="warning" startIcon={<RestartAlt />}>恢复并重启</Button>
        </DialogActions>
      </Dialog>

      <Dialog open={confirmDialog === 'rollback'} onClose={() => setConfirmDialog(null)}>
        <DialogTitle>恢复上一版本</DialogTitle>
        <DialogContent><DialogContentText>将恢复设备自动保存的上一版本，并将当前版本保留为下一次可恢复版本。建议立即重启。</DialogContentText></DialogContent>
        <DialogActions>
          <Button onClick={() => setConfirmDialog(null)}>取消</Button>
          <Button onClick={() => void handleRollback(false)} variant="outlined" color="warning">恢复（稍后重启）</Button>
          <Button onClick={() => void handleRollback(true)} variant="contained" color="warning" startIcon={<RestartAlt />}>恢复并重启</Button>
        </DialogActions>
      </Dialog>

      <Dialog open={confirmDialog === 'cancel'} onClose={() => setConfirmDialog(null)}>
        <DialogTitle>确认取消更新</DialogTitle>
        <DialogContent><DialogContentText>这将删除已上传的待安装更新包，不会删除本机的上一版本恢复快照。</DialogContentText></DialogContent>
        <DialogActions>
          <Button onClick={() => setConfirmDialog(null)}>返回</Button>
          <Button onClick={() => void handleCancel()} variant="contained" color="error">确认取消</Button>
        </DialogActions>
      </Dialog>
    </Box>
  )
}
