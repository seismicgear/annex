/**
 * Compatibility entry point. The implementation now lives under
 * `@/voice/` (component + per-concern hooks). This shim keeps the
 * existing `import { VoicePanel } from '@/components/VoicePanel'` paths
 * — and the colocated `VoicePanel.test.tsx` — working unchanged.
 */

export { VoicePanel } from '@/voice/VoicePanel';
