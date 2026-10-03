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
    // WebView2 跟随 Edge 自动更新，目标可以很新
    target: 'chrome110',
    minify: 'esbuild',
    sourcemap: false,
    chunkSizeWarningLimit: 800,
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
});
