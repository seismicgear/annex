# ADR 0005: Client Framework Selection

**Status**: Accepted
**Date**: 2026-02-18
**Phase**: 11 (Client)

## Context

Phase 11 requires a web client that handles:
- Client-side ZK proof generation via snarkjs (WASM)
- WebSocket messaging for real-time text channels
- LiveKit WebRTC integration for voice channels
- IndexedDB key storage for persistent identity
- Presence graph visualization
- Federation-aware UI

The roadmap requires an ADR choosing between React, Solid, Svelte, or vanilla.

## Options Considered

### React + TypeScript + Vite

- **Pros**: Largest ecosystem; official LiveKit React SDK (`@livekit/components-react`); snarkjs works in any framework; most contributors are familiar with React; extensive component library ecosystem; strong TypeScript support.
- **Cons**: Larger bundle than Solid/Svelte; virtual DOM overhead (negligible for this use case).

### SolidJS + TypeScript + Vite

- **Pros**: Smaller bundles; true reactivity without virtual DOM; familiar JSX syntax.
- **Cons**: No official LiveKit SDK (would need to wrap vanilla JS SDK manually); smaller ecosystem; fewer available UI components.

### Svelte + TypeScript + Vite

- **Pros**: Compiled output is small; simple syntax; built-in state management.
- **Cons**: No official LiveKit SDK; different syntax from rest of ecosystem; compiler-based approach adds build complexity.

### Vanilla JS/TS

- **Pros**: No framework overhead; maximum control.
- **Cons**: Significant boilerplate for reactive UI; manual DOM management for a chat application is error-prone and slow to develop; no component ecosystem.

## Decision

**React + TypeScript + Vite**

The deciding factor is **LiveKit's official React SDK** (`@livekit/components-react`). Voice is a core feature of Annex, not an afterthought. Using the official SDK avoids reimplementing room management, audio track rendering, and participant state synchronization. The React ecosystem also provides the most mature IndexedDB wrappers, WebSocket state management libraries, and UI component options.

Vite is chosen over Create React App (deprecated) or Next.js (SSR not needed — this is a client-side SPA connecting to the Annex server).

### Key Dependencies

| Package | Purpose |
|---------|---------|
| `react` + `react-dom` | UI framework |
| `typescript` | Type safety |
| `vite` | Build tool and dev server |
| `snarkjs` | Client-side Groth16 proof generation |
| `circomlibjs` | Poseidon hash computation for commitments |
| `@livekit/components-react` + `livekit-client` | Voice channel WebRTC |
| `idb` | IndexedDB wrapper for key storage |
| `zustand` | Lightweight state management (no Redux overhead) |

## Consequences

- React's virtual DOM adds ~40KB gzipped to the bundle. This is acceptable given the snarkjs WASM binary (~800KB) already dominates bundle size.
- TypeScript strict mode enforced to maintain code quality consistent with the Rust backend.
- Vite's dev server provides hot module replacement for rapid development.
- The client will be a single-page application served as static files. It can be hosted anywhere (CDN, same server, etc.).
