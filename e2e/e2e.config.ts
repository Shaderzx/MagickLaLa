import { web } from '@e2e-dev/web';
import type { E2EConfig } from 'e2e';

export default {
  targets: [{ engine: web(), app: { url: 'http://127.0.0.1:7777' } }],
  maxDuration: 900_000,
} satisfies E2EConfig;