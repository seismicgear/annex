/** UI contract tests only. Real persistence and VRP negotiation are covered by
 * the unstubbed operator live lane, not these isolated component fixtures. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ServerPolicy } from '@/types';

const { getPolicy, updatePolicy } = vi.hoisted(() => ({ getPolicy: vi.fn(), updatePolicy: vi.fn() }));
vi.mock('@/lib/api', () => ({
  getPolicy, updatePolicy,
  subscribeUploadGrant: () => () => {}, getUploadGrantVersion: () => 0,
}));

const initial: ServerPolicy = {
  access_mode: 'public', access_password: '', agent_min_alignment_score: 0.06,
  agent_required_capabilities: [], default_retention_days: 30, federation_enabled: true,
  voice_enabled: true, max_members: 1000, rate_limit: { registration_limit: 5, verification_limit: 10, default_limit: 100 },
  principles: ['Respect privacy'], prohibited_actions: [], usernames_enabled: true,
  images_enabled: true, videos_enabled: true, files_enabled: true,
  max_image_size_mb: 5, max_video_size_mb: 25, max_file_size_mb: 10,
  allow_reflection_summaries: true, allow_full_knowledge: false,
  knowledge_domains_allowed: null, rtx_retention_seconds: null, max_exchange_size: 1048576,
};

async function open() {
  const { useIdentityStore } = await import('@/stores/identity');
  const { AdminPanel } = await import('./AdminPanel');
  useIdentityStore.setState({ identity: { pseudonymId: 'operator' } as never });
  render(<AdminPanel section="policy" />);
  await screen.findByRole('checkbox', { name: 'Allow Reflection Summaries' });
  return userEvent.setup();
}

describe('operator knowledge transfer policy', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getPolicy.mockResolvedValue(structuredClone(initial));
    updatePolicy.mockResolvedValue({ version_id: 'new-policy-version' });
  });
  afterEach(cleanup);

  it('keeps summaries enabled and full knowledge disabled unless explicitly selected', async () => {
    await open();
    expect(screen.getByRole('checkbox', { name: 'Allow Reflection Summaries' })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: 'Allow Full Knowledge Bundles' })).not.toBeChecked();
    expect(screen.getByText(/Existing alignment requirements and topic restrictions still apply/)).toBeVisible();
    expect(updatePolicy).not.toHaveBeenCalled();
  });

  it('submits each independent choice using the exact backend policy fields', async () => {
    const user = await open();
    await user.click(screen.getByRole('checkbox', { name: 'Allow Reflection Summaries' }));
    await user.click(screen.getByRole('checkbox', { name: 'Allow Full Knowledge Bundles' }));
    await user.click(screen.getByRole('button', { name: 'Save Policy' }));
    await waitFor(() => expect(updatePolicy).toHaveBeenCalledWith('operator', {
      ...initial, allow_reflection_summaries: false, allow_full_knowledge: true,
    }));
    await user.click(screen.getByRole('checkbox', { name: 'Allow Full Knowledge Bundles' }));
    await user.click(screen.getByRole('button', { name: 'Save Policy' }));
    await waitFor(() => expect(updatePolicy).toHaveBeenLastCalledWith('operator', {
      ...initial, allow_reflection_summaries: false, allow_full_knowledge: false,
    }));
  });

  it('retains operator choices and reports a rejected save without a success claim', async () => {
    updatePolicy.mockRejectedValue(new Error('policy update refused'));
    const user = await open();
    await user.click(screen.getByRole('checkbox', { name: 'Allow Full Knowledge Bundles' }));
    await user.click(screen.getByRole('button', { name: 'Save Policy' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('policy update refused');
    expect(screen.queryByText(/Policy updated/)).not.toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'Allow Full Knowledge Bundles' })).toBeChecked();
  });

  it('uses the backend compatibility defaults when reading a policy from an older server', async () => {
    const legacy: Partial<ServerPolicy> = { ...initial };
    delete legacy.allow_reflection_summaries; delete legacy.allow_full_knowledge;
    delete legacy.knowledge_domains_allowed; delete legacy.rtx_retention_seconds; delete legacy.max_exchange_size;
    getPolicy.mockResolvedValue(legacy);
    const user = await open();
    await user.click(screen.getByRole('button', { name: 'Save Policy' }));
    await waitFor(() => expect(updatePolicy).toHaveBeenCalledWith('operator', initial));
  });

  it('saves explicit domain, byte and lifetime restrictions without treating an empty list as unrestricted', async () => {
    const user = await open();
    await user.click(screen.getByRole('checkbox', { name: 'Restrict Knowledge Domains' }));
    await user.click(screen.getByRole('button', { name: 'Save Policy' }));
    await waitFor(() => expect(updatePolicy).toHaveBeenLastCalledWith('operator', { ...initial, knowledge_domains_allowed: [] }));
    await user.type(screen.getByRole('textbox', { name: 'Allowed Knowledge Domains' }), 'Rust, security\nrust');
    const size = screen.getByRole('spinbutton', { name: 'Maximum Knowledge Bundle Size (bytes)' });
    await user.clear(size); await user.type(size, '2048');
    await user.click(screen.getByRole('checkbox', { name: 'Limit Knowledge Retention' }));
    const retention = screen.getByRole('spinbutton', { name: 'Knowledge Retention (seconds)' });
    await user.clear(retention); await user.type(retention, '60');
    await user.click(screen.getByRole('button', { name: 'Save Policy' }));
    await waitFor(() => expect(updatePolicy).toHaveBeenLastCalledWith('operator', {
      ...initial, knowledge_domains_allowed: ['rust', 'security'], max_exchange_size: 2048, rtx_retention_seconds: 60,
    }));
  });

  it('rejects invalid limits locally and retains the form after a rejected save', async () => {
    const user = await open();
    const size = screen.getByRole('spinbutton', { name: 'Maximum Knowledge Bundle Size (bytes)' });
    await user.clear(size); await user.type(size, '1048577');
    await user.click(screen.getByRole('button', { name: 'Save Policy' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('between 1 and 1048576 bytes');
    expect(updatePolicy).not.toHaveBeenCalled();
  });
});
