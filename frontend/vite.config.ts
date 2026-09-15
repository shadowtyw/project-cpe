import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { execSync } from 'child_process'
import { readFileSync } from 'fs'
import { fileURLToPath, URL } from 'node:url'

// Read version and git info at build time
const getVersionInfo = () => {
  try {
    const version = readFileSync('../VERSION', 'utf-8').trim()
    const gitBranch = execSync('git rev-parse --abbrev-ref HEAD').toString().trim()
    const gitCommit = execSync('git rev-parse --short HEAD').toString().trim()
    return { version, gitBranch, gitCommit }
  } catch {
    return { version: '3.0.0', gitBranch: 'unknown', gitCommit: 'unknown' }
  }
}

const { version, gitBranch, gitCommit } = getVersionInfo()

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    react(),
  ],

  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
    // Ensure only one React copy is used to avoid invalid hook call issues.
    dedupe: ['react', 'react-dom'],
  },

  define: {
    __APP_VERSION__: JSON.stringify(version),
    __GIT_BRANCH__: JSON.stringify(gitBranch),
    __GIT_COMMIT__: JSON.stringify(gitCommit),
  },

  server: {
    port: 5173,
    proxy: {
      '/api': {
        target: 'http://192.168.67.1',
        changeOrigin: true,
      },
    },
  },

  // 剥离 console.* 与 debugger：设备端没有控制台可看，留在产物里只是白白增加
  // 体积并可能泄露运行时数据。这是顶层 esbuild 选项（不是 build.esbuild），
  // 对 dev 与 build 的 transform 阶段都生效。
  esbuild: {
    drop: ['console', 'debugger'],
    // 移除第三方许可长注释，进一步压缩产物
    legalComments: 'none',
  },

  build: {
    target: 'es2020',
    reportCompressedSize: false,
    // 生产构建绝不产出 sourcemap：.map 文件会把前端体积翻数倍，
    // 且会把源码结构（含调试期注释、变量名）随 OTA 包分发到设备。
    sourcemap: false,
    // esbuild 是 Vite 默认且最快的压缩器，与上面的 drop 配合剥离调试代码。
    minify: 'esbuild',
    chunkSizeWarningLimit: 600,
    rollupOptions: {
      output: {
        // 显式拆分 vendor：版本升级后浏览器仍能命中未变更库的缓存，
        // 同时避免单个巨型 chunk 撑爆 24MB 根分区的写入峰值。
        manualChunks: {
          react: ['react', 'react-dom', 'react-router-dom'],
          mui: ['@mui/material', '@mui/icons-material'],
          query: ['@tanstack/react-query'],
        },
      },
    },
  },
})
