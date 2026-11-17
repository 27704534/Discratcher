import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    port: 1437,
    strictPort: true,
    // The Rust build writes into target/ while the dev server watches. On Windows that
    // makes the watcher fail with EBUSY on a locked build script and takes the whole
    // dev server down with it.
    watch: { ignored: ['**/target/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: { target: 'es2022', chunkSizeWarningLimit: 1200 },
})
