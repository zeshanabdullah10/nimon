import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { viteSingleFile } from 'vite-plugin-singlefile'
import path from 'node:path'

// Single-file build: everything inlined into index.html so the hub's
// existing `/` handler can serve the app with no asset routes
// (crates/nimon-hub/src/server/mod.rs: include_str!("../../../../web/dist/index.html")).
//
// `npm run dev` proxies the API to a hub; override with NIMON_HUB=http://host:port.
const hub = process.env.NIMON_HUB ?? 'http://127.0.0.1:9090'

export default defineConfig({
  plugins: [react(), viteSingleFile()],
  resolve: { alias: { '@': path.resolve(__dirname, 'src') } },
  build: { outDir: 'dist', emptyOutDir: true, target: 'es2020' },
  server: {
    proxy: {
      '/api': { target: hub, changeOrigin: true },
      '/health': { target: hub, changeOrigin: true },
    },
  },
})
