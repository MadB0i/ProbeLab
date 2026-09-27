/**
 * The agent catalog panel.
 *
 * This component used to be a hardcoded array. The reason that mattered is not tidiness: the
 * dropdown used to be able to offer an agent the backend then refused, because the same agent list was
 * maintained in the React component, the Rust mock enum, the IPC allowlist, and the reference policies
 * — and only the first of those was visible while writing UI code.
 *
 * Everything here now comes from `listAgents()`, so the panel cannot drift from what the backend can
 * actually build.
 */

import type { AgentSummary } from "./api";

/**
 * The colour for a category, chosen to encode what the number *means* rather than to look tidy.
 *
 * The distinction the palette has to carry is between "this measures a model" and "this measures the
 * lab's own boundary." A model result and a sandbox probe result are not comparable numbers, and
 * rendering them in the same neutral style invites exactly the comparison they cannot support.
 */
const CATEGORY_TONE: Record<string, string> = {
  target_model: "tone-model",
  reference_ceiling: "tone-ceiling",
  deterministic_baseline: "tone-baseline",
  policy_agent: "tone-policy",
  protocol_probe: "tone-probe",
  security_probe: "tone-probe",
};

function toneFor(agent: AgentSummary): string {
  if (agent.measuresAModel) {
    return "tone-model";
  }
  return CATEGORY_TONE[agent.category] ?? "tone-neutral";
}

/**
 * What a score from this agent is allowed to be called.
 *
 * The wording is the point. A security probe's number is not "accuracy" — it is a count of boundary
 * attempts, and calling it anything else is how a lab ends up claiming a sandbox holds because a model
 * misbehaved.
 */
function measurementLabel(agent: AgentSummary): string {
  if (agent.measuresAModel) {
    return "Model capability";
  }
  if (agent.category === "security_probe") {
    return "Boundary attempts (measures the sandbox, not the model)";
  }
  if (agent.category === "protocol_probe") {
    return "Protocol adherence (measures response format)";
  }
  if (agent.category === "reference_ceiling") {
    return "Ceiling (trusted policy, not a model)";
  }
  return "Lab baseline (infrastructure, not a model)";
}

function groupKey(agent: AgentSummary): string {
  return agent.measuresAModel ? "model" : "infrastructure";
}

const GROUP_TITLES: Record<string, string> = {
  model: "Target models",
  infrastructure: "Lab infrastructure — not models",
};

function AgentRow({ agent }: { agent: AgentSummary }) {
  return (
    <li className={`agent-row ${toneFor(agent)}`}>
      <div className="agent-row-head">
        <span className="agent-name">{agent.name}</span>
        <span className="agent-category">{agent.categoryLabel}</span>
      </div>
      <p className="agent-description">{agent.description}</p>
      <dl className="agent-facts">
        <div>
          <dt>Reports</dt>
          <dd>{measurementLabel(agent)}</dd>
        </div>
        <div>
          <dt>Executes</dt>
          <dd>
            {agent.requiresNetwork ? "Remote model" : "In-process, deterministic"}
          </dd>
        </div>
        <div>
          <dt>Applies to</dt>
          <dd>
            {agent.applicableBenchmarks.length === 0
              ? "Every benchmark"
              : agent.applicableBenchmarks.join(", ")}
          </dd>
        </div>
      </dl>
    </li>
  );
}

export function AgentCatalog({ agents }: { agents: AgentSummary[] }) {
  if (agents.length === 0) {
    return (
      <p className="empty">
        No agents are registered. This is a fault in the build, not an empty state — the registry
        always ships baselines.
      </p>
    );
  }

  const groups = new Map<string, AgentSummary[]>();
  for (const agent of agents) {
    const key = groupKey(agent);
    const existing = groups.get(key);
    if (existing) {
      existing.push(agent);
    } else {
      groups.set(key, [agent]);
    }
  }

  return (
    <div className="agent-catalog">
      <p className="panel-note">
        {agents.filter((agent) => agent.measuresAModel).length === 0
          ? "No target model is connected. Every agent below is lab infrastructure, so nothing here "
            + "can produce a score about a model's ability."
          : `${agents.filter((a) => a.measuresAModel).length} target model(s) connected. The rest are ` +
            "infrastructure and their numbers measure the lab, not a model."}
      </p>
      {[...groups.entries()].map(([key, members]) => (
        <section key={key} className="agent-group">
          <h3 className="agent-group-title">{GROUP_TITLES[key] ?? key}</h3>
          <ul className="agent-list">
            {members.map((agent) => (
              <AgentRow key={agent.id} agent={agent} />
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}
