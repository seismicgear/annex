/**
 * Compatibility entry point. The implementation now lives under
 * `@/app/AppShell` (component) plus the per-concern hooks in `@/app/`.
 * This file keeps the original `import App from './App'` and named
 * `ReconnectionBanner` import paths working without changing call sites.
 */

import './App.css';
import App, { ReconnectionBanner } from './app/AppShell';

export { ReconnectionBanner };
export default App;
