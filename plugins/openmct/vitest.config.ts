// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    // Tests run in the Node environment. The DOM tests create their own jsdom window in a
    // `beforeAll` hook (test/dom.ts) instead of using `environment: 'jsdom'`. Vitest loads a
    // test environment while the worker starts, under a fixed 60 s start timeout that cannot be
    // configured; on slow or virus-scanned Windows drives the first jsdom import alone can take
    // longer, which failed the whole run with "[vitest-pool-runner]: Timeout waiting for worker
    // to respond". Loading jsdom inside a hook moves it under `hookTimeout` instead.
    environment: 'node',
    include: ['test/**/*.test.ts'],
    pool: 'forks',
    hookTimeout: 180_000,
  },
});
