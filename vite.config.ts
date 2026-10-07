import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

const root = dirname(fileURLToPath(import.meta.url));

// 所有窗口共用一个 index.html，按窗口标签选视图（src/main.tsx），每个视图按需懒加载。
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: { '@': resolve(root, 'src') },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: false,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    // Windows：WebView2 跟随 Edge 自动更新，目标可以很新。
    // macOS：系统自带的 WebKit 跟着系统版本走，按最低支持的 macOS 13（Safari 16）出，
    // esbuild 会顺带给 backdrop-filter 这类属性补上 -webkit- 前缀
    target: process.env.TAURI_ENV_PLATFORM === 'darwin' ? 'safari16' : 'chrome110',
    minify: 'esbuild',
    sourcemap: false,
    chunkSizeWarningLimit: 800,
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
});
