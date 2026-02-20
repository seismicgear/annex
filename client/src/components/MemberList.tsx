/**
 * Member list component — shows participants in the current server.
 *
 * Displays participant type indicators, online/offline status,
 * and allows clicking agents to inspect their capabilities.
 */

import { useEffect, useState, useCallback } from 'react';
import * as api from '@/lib/api';
import type { AgentInfo, ServerSummary, ParticipantType } from '@/types';

const TYPE_LABELS: Record<ParticipantType, string> = {
  HUMAN: 'Human',
  AI_AGENT: 'Agent',
  COLLECTIVE: 'Collective',
  BRIDGE: 'Bridge',
  SERVICE: 'Service',
};

function AgentDetail({ agent, onClose }: { agent: AgentInfo; onClose: () => void }) {
  return (
    <div className="agent-detail-overlay" onClick={onClose}>
      <div className="agent-detail" onClick={(e) => e.stopPropagation()}>
        <h3>Agent: {agent.pseudonym_id.slice(0, 16)}...</h3>
        <dl>
          <dt>Alignment</dt>
          <dd className={`alignment-${agent.alignment_status.toLowerCase()}`}>
            {agent.alignment_status}
          </dd>
          <dt>Transfer Scope</dt>
          <dd>{agent.transfer_scope}</dd>
          <dt>Reputation</dt>
          <dd>{agent.reputation_score.toFixed(2)}</dd>
          <dt>Capabilities</dt>
          <dd>
            {agent.capabilities.length > 0 ? (
              <ul>
                {agent.capabilities.map((c, i) => (
                  <li key={i}>{c}</li>
                ))}
              </ul>
            ) : (
              'None declared'
            )}
          </dd>
        </dl>
        <button onClick={onClose}>Close</button>
      </div>
    </div>
  );
}

export function MemberList() {
  const [summary, setSummary] = useState<ServerSummary | null>(null);
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [selectedAgent, setSelectedAgent] = useState<AgentInfo | null>(null);

  useEffect(() => {
    api.getServerSummary().then(setSummary).catch(console.error);
    api.getPublicAgents().then((r) => setAgents(r.agents)).catch(console.error);
  }, []);

  const handleAgentClick = useCallback((agent: AgentInfo) => {
    setSelectedAgent(agent);
  }, []);

  return (
    <aside className="member-list">
      {summary && (
        <div className="server-summary">
          <h3>{summary.label}</h3>
          <div className="member-counts">
            {Object.entries(summary.members_by_type).map(([type, count]) => (
              <div key={type} className="member-count">
                <span className="type-label">{TYPE_LABELS[type as ParticipantType] ?? type}</span>
                <span className="count">{count}</span>
              </div>
            ))}
          </div>
          <div className="stats">
            <span>{summary.channel_count} channels</span>
            <span>{summary.federation_peer_count} peers</span>
          </div>
        </div>
      )}

      {agents.length > 0 && (
        <div className="agent-list">
          <h4>Active Agents</h4>
          {agents.map((agent) => (
            <button
              key={agent.pseudonym_id}
              className={`agent-item alignment-${agent.alignment_status.toLowerCase()}`}
              onClick={() => handleAgentClick(agent)}
            >
              <span className="agent-name">{agent.pseudonym_id.slice(0, 12)}...</span>
              <span className="agent-badge">{agent.alignment_status}</span>
            </button>
          ))}
        </div>
      )}

      {selectedAgent && (
        <AgentDetail agent={selectedAgent} onClose={() => setSelectedAgent(null)} />
      )}
    </aside>
  );
}
