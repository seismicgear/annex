import { describe, it, expect, beforeEach } from 'vitest';
import {
  STARTUP_MODE_STORAGE_KEY,
  clearWebStartupMode,
  loadWebStartupMode,
  saveWebStartupMode,
} from './startup-prefs';

describe('startup-prefs', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('save/load/clear all use the shared startup mode storage key', () => {
    saveWebStartupMode({ mode: 'remote', server_url: 'https://annex.example.com' });

    expect(localStorage.getItem(STARTUP_MODE_STORAGE_KEY)).toBe(
      JSON.stringify({ mode: 'remote', server_url: 'https://annex.example.com' }),
    );
    expect(loadWebStartupMode()).toEqual({ mode: 'remote', server_url: 'https://annex.example.com' });

    clearWebStartupMode();

    expect(localStorage.getItem(STARTUP_MODE_STORAGE_KEY)).toBeNull();
    expect(loadWebStartupMode()).toBeNull();
  });
});
