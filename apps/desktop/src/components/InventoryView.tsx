// SPDX-License-Identifier: MIT OR Apache-2.0

import { memo, useEffect, useMemo, useState } from "react";
import {
  Background,
  Controls,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { api, errorMessage } from "../api";
import { money, relativeTime } from "../format";
import { NODE_H, NODE_W, layout } from "../graphLayout";
import type {
  Application,
  Facet,
  Graph,
  GraphEdge,
  GraphFilter,
  GraphNode,
  InventoryFacets,
  InventoryScan,
  RelationKind,
} from "../types";
import { MultiSelect } from "./MultiSelect";
import { Callout } from "./Callout";

const SHORT: Record<string, string> = {
  "cloudfront:distribution": "CF",
  "apigateway:restapi": "API",
  "apigateway:api": "API",
  "elasticloadbalancing:loadbalancer": "LB",
  "elasticloadbalancing:targetgroup": "TG",
  "ec2:instance": "EC2",
  "ecs:cluster": "ECS",
  "ecs:service": "SVC",
  "eks:cluster": "EKS",
  "lambda:function": "λ",
  "ec2:volume": "EBS",
  "rds:db": "RDS",
  "rds:cluster": "RDS",
  "dynamodb:table": "DDB",
  "elasticache:cluster": "EC",
  "s3:bucket": "S3",
  "sqs:queue": "SQS",
  "sns:topic": "SNS",
  "ec2:natgateway": "NAT",
  "ec2:elastic-ip": "EIP",
  "ec2:vpc": "VPC",
  "ec2:subnet": "SN",
  "ec2:security-group": "SG",
  "ec2:network-interface": "ENI",
  "lightsail:loadbalancer": "LSLB",
  "lightsail:instance": "LS",
  "lightsail:disk": "LSD",
  "lightsail:database": "LSDB",
  "lightsail:static-ip": "LSIP",
};

const KIND_LABEL: Record<RelationKind, string> = {
  routes_to: "routes to",
  attached: "attached",
  triggers: "triggers",
  publishes_to: "publishes to",
  in_network: "in network",
  secured_by: "secured by",
  member_of: "member of",
  owns: "owns",
};

/** Sequential single-hue ramp for the cost overlay (low → high), per theme. */
const COST_STEPS = 5;

const FIT = { padding: 0.12, maxZoom: 1.1 };

const EMPTY_FILTER: GraphFilter = { accounts: [], regions: [], vpcs: [], types: [], tag: null, include_network: false };

interface NodeData extends Record<string, unknown> {
  node: GraphNode;
  costStep: number | null;
  unit: string;
  dimmed: boolean;
  match: boolean;
}

const ResourceNode = memo(function ResourceNode({ data, selected }: NodeProps<Node<NodeData>>) {
  const { node, costStep, unit, dimmed, match } = data;
  const state = (node.attrs.state ?? node.attrs.status) as string | undefined;
  return (
    <div
      className={[
        "rnode",
        selected ? "selected" : "",
        dimmed ? "dimmed" : "",
        node.context ? "context" : "",
        match ? "match" : "",
      ].join(" ")}
      style={{ width: NODE_W, height: NODE_H }}
      title={node.id}
    >
      <Handle type="target" position={Position.Left} isConnectable={false} />
      {costStep !== null && <span className="rnode-cost" style={{ background: `var(--cost-${costStep + 1})` }} />}
      <span className="rnode-type">{SHORT[node.resource_type] ?? "•"}</span>
      <span className="rnode-text">
        <span className="rnode-label">{node.label}</span>
        <span className="rnode-sub">
          {node.type_label}
          {state ? ` · ${state}` : ""}
        </span>
      </span>
      {node.cost !== null && <span className="rnode-money">{money(node.cost, unit, true)}</span>}
      <Handle type="source" position={Position.Right} isConnectable={false} />
    </div>
  );
});

const nodeTypes = { resource: ResourceNode };

/** Quantile bucket of each node's cost, so a few large services don't wash out the rest. */
function costSteps(nodes: GraphNode[]): Map<string, number> {
  const costs = nodes.map((n) => n.cost).filter((c): c is number => c !== null && c > 0).sort((a, b) => a - b);
  const steps = new Map<string, number>();
  if (costs.length === 0) return steps;
  for (const n of nodes) {
    if (n.cost === null || n.cost <= 0) continue;
    const rank = costs.findIndex((c) => c >= n.cost!);
    steps.set(n.id, Math.min(COST_STEPS - 1, Math.floor((rank / costs.length) * COST_STEPS)));
  }
  return steps;
}

interface Props {
  workspaceId: string;
  dataVersion: number;
  costTagKeys: string[];
  onSync: () => void;
  syncing: boolean;
  onExplain: (prompt: string) => void;
}

export function InventoryView(props: Props) {
  return (
    <ReactFlowProvider>
      <Inventory {...props} />
    </ReactFlowProvider>
  );
}

function Inventory({ workspaceId, dataVersion, costTagKeys, onSync, syncing, onExplain }: Props) {
  const [mode, setMode] = useState<"graph" | "applications">("graph");
  const [filter, setFilter] = useState<GraphFilter>(EMPTY_FILTER);
  const [facets, setFacets] = useState<InventoryFacets | null>(null);
  const [tagValues, setTagValues] = useState<Facet[]>([]);
  const [graph, setGraph] = useState<Graph | null>(null);
  const [scans, setScans] = useState<InventoryScan[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [overlay, setOverlay] = useState(true);
  const [search, setSearch] = useState("");
  const [matchIndex, setMatchIndex] = useState(0);
  const [showWarnings, setShowWarnings] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const flow = useReactFlow();

  useEffect(() => {
    let cancelled = false;
    Promise.all([api.inventoryFacets(workspaceId), api.inventoryScans(workspaceId)]).then(
      ([f, s]) => {
        if (cancelled) return;
        setFacets(f);
        setScans(s);
      },
      (e) => !cancelled && setError(errorMessage(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [workspaceId, dataVersion]);

  useEffect(() => {
    let cancelled = false;
    api.inventoryGraph(workspaceId, filter).then(
      (g) => !cancelled && setGraph(g),
      (e) => !cancelled && setError(errorMessage(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [workspaceId, filter, dataVersion]);

  const tagKey = filter.tag?.key ?? null;
  useEffect(() => {
    if (!tagKey) return;
    let cancelled = false;
    api.tagValues(workspaceId, tagKey).then((v) => !cancelled && setTagValues(v), (e) => !cancelled && setError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [workspaceId, tagKey]);

  const positions = useMemo(() => (graph ? layout(graph.nodes, graph.edges) : new Map()), [graph]);

  // Re-fit whenever the filtered graph changes, not only on first render.
  useEffect(() => {
    if (!graph) return;
    const id = requestAnimationFrame(() => void flow.fitView(FIT));
    return () => cancelAnimationFrame(id);
  }, [graph, flow]);
  const steps = useMemo(() => (graph && overlay ? costSteps(graph.nodes) : new Map<string, number>()), [graph, overlay]);
  const byId = useMemo(() => new Map(graph?.nodes.map((n) => [n.id, n])), [graph]);

  const matches = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q || !graph) return [];
    return graph.nodes
      .filter((n) => n.label.toLowerCase().includes(q) || n.native_id.toLowerCase().includes(q) || n.id.toLowerCase().includes(q))
      .map((n) => n.id);
  }, [graph, search]);

  const connected = useMemo(() => {
    if (!selected || !graph) return null;
    const ids = new Set([selected]);
    for (const e of graph.edges) {
      if (e.source === selected) ids.add(e.target);
      if (e.target === selected) ids.add(e.source);
    }
    return ids;
  }, [graph, selected]);

  const nodes: Node<NodeData>[] = useMemo(() => {
    if (!graph) return [];
    const matchSet = new Set(matches);
    return graph.nodes.map((n) => ({
      id: n.id,
      type: "resource",
      position: positions.get(n.id) ?? { x: 0, y: 0 },
      selected: n.id === selected,
      data: {
        node: n,
        costStep: overlay ? (steps.get(n.id) ?? null) : null,
        unit: graph.unit,
        dimmed: connected !== null && !connected.has(n.id),
        match: matchSet.has(n.id),
      },
    }));
  }, [graph, positions, steps, overlay, selected, connected, matches]);

  const edges: Edge[] = useMemo(
    () =>
      (graph?.edges ?? []).map((e, i) => {
        const active = selected !== null && (e.source === selected || e.target === selected);
        return {
          id: `${i}`,
          source: e.source,
          target: e.target,
          animated: active,
          className: active ? "redge active" : connected ? "redge faded" : "redge",
        };
      }),
    [graph, selected, connected],
  );

  const focus = (id: string) => {
    const p = positions.get(id);
    setSelected(id);
    if (p) void flow.setCenter(p.x + NODE_W / 2, p.y + NODE_H / 2, { zoom: 1.1, duration: 300 });
  };

  const nextMatch = () => {
    if (matches.length === 0) return;
    const i = matchIndex % matches.length;
    focus(matches[i]);
    setMatchIndex(i + 1);
  };

  const set = (patch: Partial<GraphFilter>) => {
    setSelected(null);
    setFilter((f) => ({ ...f, ...patch }));
  };

  const warnings = scans.flatMap((s) => s.warnings);
  const lastScan = scans.map((s) => s.scanned_at).sort().at(-1) ?? null;

  if (facets && facets.total === 0) {
    return (
      <section className="empty">
        <h2>No resources discovered yet</h2>
        <p className="muted">
          Sync to scan every enabled region with read-only <code>Describe</code>/<code>List</code> calls. These calls are free.
          The connected role needs the AWS managed <code>ViewOnlyAccess</code> policy (or <code>policies/afinin-readonly.json</code>).
        </p>
        <button className="primary" onClick={onSync} disabled={syncing}>
          {syncing ? "Syncing…" : "Sync now"}
        </button>
      </section>
    );
  }

  const selectedNode = selected ? byId.get(selected) : undefined;

  return (
    <div className="inventory">
      {error && (
        <Callout tone="danger" title="Couldn’t load the inventory" onDismiss={() => setError(null)} dismissLabel="Dismiss">
          {error}
        </Callout>
      )}

      <section className="filters">
        <div className="segmented" role="group" aria-label="View">
          <button className={mode === "graph" ? "active" : ""} onClick={() => setMode("graph")}>
            Graph
          </button>
          <button className={mode === "applications" ? "active" : ""} onClick={() => setMode("applications")}>
            Applications
          </button>
        </div>
        {mode === "graph" && facets && (
          <>
            <MultiSelect label="Accounts" options={facets.accounts} selected={filter.accounts} onChange={(accounts) => set({ accounts })} />
            <MultiSelect label="Regions" options={facets.regions} selected={filter.regions} onChange={(regions) => set({ regions })} />
            <MultiSelect label="VPCs" options={facets.vpcs} selected={filter.vpcs} onChange={(vpcs) => set({ vpcs })} />
            <MultiSelect label="Types" options={facets.types} selected={filter.types} onChange={(types) => set({ types })} />
            <select
              aria-label="Tag key"
              value={filter.tag?.key ?? ""}
              onChange={(e) => set({ tag: e.target.value ? { key: e.target.value, value: null } : null })}
            >
              <option value="">Any tag</option>
              {facets.tag_keys.map((t) => (
                <option key={t.key} value={t.key}>
                  Tag: {t.label}
                </option>
              ))}
            </select>
            {filter.tag && (
              <select
                aria-label="Tag value"
                value={filter.tag.value ?? ""}
                onChange={(e) => set({ tag: { key: filter.tag!.key, value: e.target.value || null } })}
              >
                <option value="">Any value</option>
                {tagValues.map((v) => (
                  <option key={v.key} value={v.key}>
                    {v.label} ({v.count})
                  </option>
                ))}
              </select>
            )}
            <label className="check inline">
              <input
                type="checkbox"
                checked={filter.include_network}
                onChange={(e) => set({ include_network: e.target.checked })}
              />
              Network
            </label>
            <label className="check inline">
              <input type="checkbox" checked={overlay} onChange={(e) => setOverlay(e.target.checked)} />
              Cost overlay
            </label>
            <span className="spacer" />
            <input
              className="search"
              placeholder="Search name, ID or ARN…"
              value={search}
              onChange={(e) => {
                setSearch(e.target.value);
                setMatchIndex(0);
              }}
              onKeyDown={(e) => e.key === "Enter" && nextMatch()}
            />
            {search && <span className="muted small">{matches.length} found</span>}
          </>
        )}
      </section>

      <p className="muted small scan-line">
        {graph && (
          <>
            Showing {graph.nodes.length.toLocaleString()} of {graph.total_resources.toLocaleString()} resources ·{" "}
          </>
        )}
        {(() => {
          const n = new Set(scans.map((s) => s.scan_region)).size;
          return `${n} region${n === 1 ? "" : "s"} scanned ${relativeTime(lastScan)}`;
        })()}
        {warnings.length > 0 && (
          <>
            {" · "}
            <button className="link" onClick={() => setShowWarnings((v) => !v)}>
              {warnings.length} warning{warnings.length === 1 ? "" : "s"}
            </button>
          </>
        )}
      </p>
      {showWarnings && (
        <ul className="warnings">
          {warnings.map((w, i) => (
            <li key={i}>{w}</li>
          ))}
        </ul>
      )}

      {mode === "applications" ? (
        <ApplicationsPanel
          workspaceId={workspaceId}
          tagKeys={facets?.tag_keys ?? []}
          costTagKeys={costTagKeys}
          dataVersion={dataVersion}
          onOpen={(key, value) => {
            set({ tag: { key, value }, accounts: [], regions: [], vpcs: [], types: [] });
            setMode("graph");
          }}
        />
      ) : (
        <div className="graph-area">
          <div className="graph-canvas">
            <ReactFlow
              nodes={nodes}
              edges={edges}
              nodeTypes={nodeTypes}
              onNodeClick={(_, n) => setSelected(n.id)}
              onPaneClick={() => setSelected(null)}
              nodesDraggable={false}
              nodesConnectable={false}
              onlyRenderVisibleElements
              minZoom={0.05}
              fitView
              fitViewOptions={FIT}
              proOptions={{ hideAttribution: true }}
            >
              <Background gap={24} size={1} color="var(--chart-grid)" />
              <Controls showInteractive={false} />
              {graph && graph.nodes.length > 60 && (
                <MiniMap
                  pannable
                  zoomable
                  className="minimap"
                  nodeStrokeWidth={0}
                  nodeColor="var(--edge)"
                  maskColor="var(--minimap-mask)"
                />
              )}
            </ReactFlow>
            {overlay && graph && graph.nodes.some((n) => n.cost !== null) && (
              <div className="legend-overlay">
                <span>{graph.cost_basis === "service" ? "Service cost per account, 30 days" : "Resource cost, 30 days"}</span>
                <span className="ramp">
                  {Array.from({ length: COST_STEPS }, (_, i) => (
                    <span key={i} style={{ background: `var(--cost-${i + 1})` }} />
                  ))}
                </span>
                <span className="muted">low → high</span>
              </div>
            )}
          </div>
          {selectedNode && graph && (
            <Details
              node={selectedNode}
              graph={graph}
              byId={byId}
              onSelect={focus}
              onClose={() => setSelected(null)}
              onExplain={() =>
                onExplain(`Explain the resource ${selectedNode.id}: what it is, what it is connected to and what it costs.`)
              }
            />
          )}
        </div>
      )}
    </div>
  );
}

function Details({
  node,
  graph,
  byId,
  onSelect,
  onClose,
  onExplain,
}: {
  node: GraphNode;
  graph: Graph;
  byId: Map<string, GraphNode>;
  onSelect: (id: string) => void;
  onClose: () => void;
  onExplain: () => void;
}) {
  const links = graph.edges
    .filter((e: GraphEdge) => e.source === node.id || e.target === node.id)
    .map((e) => {
      const outgoing = e.source === node.id;
      const other = byId.get(outgoing ? e.target : e.source);
      return { e, outgoing, other };
    })
    .filter((l) => l.other);

  return (
    <aside className="details" aria-label="Resource details">
      <header>
        <div>
          <div className="muted small">{node.type_label}</div>
          <h2>{node.label}</h2>
        </div>
        <button className="ghost icon" onClick={onClose} aria-label="Close">
          ×
        </button>
      </header>
      <button className="ghost explain" onClick={onExplain}>
        Explain this resource
      </button>
      <dl>
        <dt>ID</dt>
        <dd>
          <code>{node.native_id}</code>
        </dd>
        <dt>ARN</dt>
        <dd>
          <code className="wrap">{node.id}</code>
          <button className="ghost small" onClick={() => void navigator.clipboard?.writeText(node.id)}>
            Copy
          </button>
        </dd>
        <dt>Account</dt>
        <dd>
          <code>{node.account_id}</code>
        </dd>
        <dt>Region</dt>
        <dd>{node.region}</dd>
        {node.vpc_id && (
          <>
            <dt>VPC</dt>
            <dd>
              <code>{node.vpc_id}</code>
            </dd>
          </>
        )}
        {node.cost !== null && (
          <>
            <dt>{graph.cost_basis === "service" ? "Service cost" : "Cost"}</dt>
            <dd>
              {money(node.cost, graph.unit)} <span className="muted small">last 30 days</span>
              {graph.cost_basis === "service" && (
                <div className="muted small">Whole service in this account. Per-resource cost needs CUR.</div>
              )}
            </dd>
          </>
        )}
      </dl>

      {links.length > 0 && (
        <>
          <h3>Connections ({links.length})</h3>
          <ul className="links">
            {links.map(({ e, outgoing, other }, i) => (
              <li key={i}>
                <button className="link" onClick={() => onSelect(other!.id)}>
                  <span className="muted small">
                    {outgoing ? KIND_LABEL[e.kind] : `← ${KIND_LABEL[e.kind]}`}
                  </span>{" "}
                  {other!.label} <span className="muted small">{other!.type_label}</span>
                </button>
              </li>
            ))}
          </ul>
        </>
      )}

      {Object.keys(node.attrs).length > 0 && (
        <>
          <h3>Details</h3>
          <dl>
            {Object.entries(node.attrs).map(([k, v]) => (
              <div key={k} className="kv">
                <dt>{k.replace(/_/g, " ")}</dt>
                <dd>{typeof v === "string" ? v : JSON.stringify(v)}</dd>
              </div>
            ))}
          </dl>
        </>
      )}

      {Object.keys(node.tags).length > 0 && (
        <>
          <h3>Tags</h3>
          <dl>
            {Object.entries(node.tags).map(([k, v]) => (
              <div key={k} className="kv">
                <dt>{k}</dt>
                <dd>{v}</dd>
              </div>
            ))}
          </dl>
        </>
      )}
    </aside>
  );
}

function ApplicationsPanel({
  workspaceId,
  tagKeys,
  costTagKeys,
  dataVersion,
  onOpen,
}: {
  workspaceId: string;
  tagKeys: Facet[];
  costTagKeys: string[];
  dataVersion: number;
  onOpen: (key: string, value: string) => void;
}) {
  const preferred = ["app", "application", "service", "project", "env"];
  const initial =
    costTagKeys.find((k) => tagKeys.some((t) => t.key === k)) ??
    preferred.find((p) => tagKeys.some((t) => t.key.toLowerCase() === p)) ??
    tagKeys[0]?.key ??
    "";
  const [key, setKey] = useState(initial);
  const [apps, setApps] = useState<Application[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!key) return;
    let cancelled = false;
    api.applications(workspaceId, key).then((a) => !cancelled && setApps(a), (e) => !cancelled && setError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [workspaceId, key, dataVersion]);

  if (tagKeys.length === 0) {
    return (
      <section className="empty">
        <h2>No tags found</h2>
        <p className="muted">
          Applications group resources by a tag such as <code>app</code> or <code>env</code>. Tag your resources and sync
          again.
        </p>
      </section>
    );
  }

  const withCost = costTagKeys.includes(key);
  return (
    <section className="panel">
      <header className="panel-header">
        <label className="inline-label">
          Group by tag{" "}
          <select value={key} onChange={(e) => setKey(e.target.value)}>
            {tagKeys.map((t) => (
              <option key={t.key} value={t.key}>
                {t.label}
              </option>
            ))}
          </select>
        </label>
        {!withCost && (
          <span className="muted small">
            Add <code>{key}</code> to the cost allocation tag keys in Settings to see cost per application.
          </span>
        )}
      </header>
      {error && (
        <Callout tone="danger" title="Couldn’t load applications">
          {error}
        </Callout>
      )}
      <table className="breakdown">
        <thead>
          <tr>
            <th>{key}</th>
            <th className="num">Resources</th>
            <th>Mostly</th>
            {withCost && <th className="num">Cost, 30 days</th>}
          </tr>
        </thead>
        <tbody>
          {(apps ?? []).map((a) => {
            const top = Object.entries(a.resource_types)
              .sort((x, y) => y[1] - x[1])
              .slice(0, 3)
              .map(([t, n]) => `${n} ${SHORT[t] ?? t}`)
              .join(", ");
            return (
              <tr
                key={a.value}
                className={a.value && a.resource_count > 0 ? "clickable" : ""}
                onClick={() => a.value && a.resource_count > 0 && onOpen(key, a.value)}
              >
                <td>{a.value || <span className="muted">(untagged)</span>}</td>
                <td className="num">{a.resource_count}</td>
                <td className="muted small">{top}</td>
                {withCost && <td className="num">{a.cost === null ? "—" : money(a.cost)}</td>}
              </tr>
            );
          })}
        </tbody>
      </table>
    </section>
  );
}
