import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// The dev server proxies API calls to a running backend. It defaults to the
// development backend on 3877; COMPANION_API_TARGET points it at another one,
// such as the app already running on 5173, without starting a second backend.
const apiTarget = (process.env.COMPANION_API_TARGET ?? '').trim() || 'http://127.0.0.1:3877';

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      '/api': apiTarget,
    },
  },
});
