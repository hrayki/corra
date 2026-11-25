import { defineConfig } from 'vite'
import solid from 'vite-plugin-solid'
import tailwindcss from '@tailwindcss/vite'

const hub = process.env.CORRA_HUB ?? 'http://127.0.0.1:7810'

export default defineConfig({
  plugins: [solid(), tailwindcss()],
  server: {
    port: 7820,
    strictPort: true,
    proxy: { '/app': { target: hub.replace(/^http/, 'ws'), ws: true }, '/auth': hub },
  },
  build: { target: 'es2023', sourcemap: false },
})
