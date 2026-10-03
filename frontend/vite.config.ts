import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { VitePWA } from 'vite-plugin-pwa'
import path from 'path'

export default defineConfig({
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  plugins: [
    react(),
    tailwindcss(),
    VitePWA({
      registerType: 'autoUpdate',
      devOptions: {
        enabled: true,
      },
      workbox: {
        // Only pre-download what the app needs to start. lucide's DynamicIcon emits one chunk
        // per icon (~1,600 files); pre-caching them all stops the service worker installing,
        // so those are cached the first time they're used instead.
        globPatterns: [
          '*.{html,png,svg,ico,webmanifest}',
          'assets/index-*.{js,css}',
          'assets/workbox-window*.js',
        ],
        // Server routes are never the app's page: downloads (like the database backup) and
        // sign-in flows must reach the server rather than fall back to index.html.
        navigateFallbackDenylist: [/^\/api\//, /^\/mcp/, /^\/oauth\/(token|register|revoke)/, /^\/\.well-known\//],
        runtimeCaching: [
          {
            // Hashed filenames never change content, so cached copies can be reused as-is.
            urlPattern: ({ url }) => url.pathname.startsWith('/assets/'),
            handler: 'CacheFirst',
            options: { cacheName: 'assets', expiration: { maxEntries: 300 } },
          },
        ],
      },
      includeAssets: ['logo.png', 'favicon.svg'],
      manifest: {
        name: 'PennyChest',
        short_name: 'PennyChest',
        description: 'Personal finance tracker',
        theme_color: '#2A6B2A',
        background_color: '#ffffff',
        display: 'standalone',
        start_url: '/',
        icons: [
          {
            src: 'pwa-192x192.png',
            sizes: '192x192',
            type: 'image/png',
          },
          {
            src: 'pwa-512x512.png',
            sizes: '512x512',
            type: 'image/png',
            purpose: 'any maskable',
          },
        ],
      },
    }),
  ],
  server: {
    proxy: {
      '/api': {
        target: 'http://api:8000',
        changeOrigin: true,
      },
    },
  },
})
