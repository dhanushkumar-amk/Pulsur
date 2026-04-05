"use client";

import type { ComponentType, ReactNode } from "react";
import { startTransition, useDeferredValue, useEffect, useState } from "react";
import {
  Area,
  AreaChart,
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

const AreaAny = Area as unknown as ComponentType<Record<string, unknown>>;
const AreaChartAny = AreaChart as unknown as ComponentType<Record<string, unknown>>;
const CartesianGridAny = CartesianGrid as unknown as ComponentType<Record<string, unknown>>;
const LineAny = Line as unknown as ComponentType<Record<string, unknown>>;
const LineChartAny = LineChart as unknown as ComponentType<Record<string, unknown>>;
const ResponsiveContainerAny =
  ResponsiveContainer as unknown as ComponentType<Record<string, unknown>>;
const TooltipAny = Tooltip as unknown as ComponentType<Record<string, unknown>>;
const XAxisAny = XAxis as unknown as ComponentType<Record<string, unknown>>;
const YAxisAny = YAxis as unknown as ComponentType<Record<string, unknown>>;

type ComponentState = "healthy" | "degraded" | "down";

type ComponentHealth = {
  component: string;
  state: ComponentState;
  detail: string;
  updated_at_ms: number;
};

type MetricsSnapshot = {
  timestamp_ms: number;
  request_count: number;
  req_per_sec: number;
  request_duration_p99_ms: number;
  error_count: number;
  error_rate: number;
  active_connections: number;
  queue_depth: number;
  cache_hit_rate: number;
  component_health: ComponentHealth[];
};

type StreamStatus = "connecting" | "live" | "reconnecting";

const STREAM_URL =
  `${process.env.NEXT_PUBLIC_OBSERVABILITY_URL ?? "http://127.0.0.1:9090"}/metrics/stream`;

const FALLBACK_COMPONENTS: ComponentHealth[] = [
  { component: "http-server", state: "healthy", detail: "Awaiting live metrics", updated_at_ms: Date.now() },
  { component: "gateway", state: "degraded", detail: "No SSE payload received yet", updated_at_ms: Date.now() },
  { component: "proxy", state: "healthy", detail: "Dashboard booted successfully", updated_at_ms: Date.now() },
  { component: "queue", state: "healthy", detail: "Waiting for queue telemetry", updated_at_ms: Date.now() },
  { component: "circuit-breaker", state: "degraded", detail: "No breaker health updates yet", updated_at_ms: Date.now() },
];

function fallbackSnapshot(): MetricsSnapshot {
  return {
    timestamp_ms: Date.now(),
    request_count: 0,
    req_per_sec: 0,
    request_duration_p99_ms: 0,
    error_count: 0,
    error_rate: 0,
    active_connections: 0,
    queue_depth: 0,
    cache_hit_rate: 0,
    component_health: FALLBACK_COMPONENTS,
  };
}

export function DashboardClient() {
  const [snapshots, setSnapshots] = useState<MetricsSnapshot[]>([fallbackSnapshot()]);
  const [streamStatus, setStreamStatus] = useState<StreamStatus>("connecting");
  const [lastUpdated, setLastUpdated] = useState<number | null>(null);

  useEffect(() => {
    const source = new EventSource(STREAM_URL);

    source.addEventListener("open", () => {
      startTransition(() => {
        setStreamStatus("live");
      });
    });

    source.addEventListener("snapshot", (event) => {
      const payload = JSON.parse((event as MessageEvent<string>).data) as MetricsSnapshot;
      startTransition(() => {
        setSnapshots((current) => [...current, payload].slice(-60));
        setLastUpdated(payload.timestamp_ms);
        setStreamStatus("live");
      });
    });

    source.addEventListener("error", () => {
      startTransition(() => {
        setStreamStatus("reconnecting");
      });
    });

    return () => {
      source.close();
    };
  }, []);

  const deferredSnapshots = useDeferredValue(snapshots);
  const latest = deferredSnapshots[deferredSnapshots.length - 1] ?? fallbackSnapshot();
  const chartData = deferredSnapshots.map((snapshot) => ({
    time: new Date(snapshot.timestamp_ms).toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    }),
    reqPerSec: Number(snapshot.req_per_sec.toFixed(2)),
    p99: Number(snapshot.request_duration_p99_ms.toFixed(2)),
    errorRate: Number((snapshot.error_rate * 100).toFixed(2)),
    activeConnections: snapshot.active_connections,
  }));

  return (
    <main className="min-h-screen overflow-hidden bg-[radial-gradient(circle_at_top_left,_rgba(250,204,21,0.18),_transparent_28%),radial-gradient(circle_at_bottom_right,_rgba(14,165,233,0.16),_transparent_32%),linear-gradient(180deg,_#f7f4ec_0%,_#fffdf7_44%,_#f2efe6_100%)] text-slate-950">
      <div className="mx-auto flex w-full max-w-7xl flex-col gap-8 px-5 py-6 sm:px-8 lg:px-10">
        <section className="grid gap-6 rounded-[2rem] border border-black/10 bg-white/70 p-6 shadow-[0_30px_80px_rgba(15,23,42,0.08)] backdrop-blur sm:p-8 lg:grid-cols-[1.4fr_0.9fr]">
          <div className="space-y-5">
            <div className="inline-flex items-center gap-2 rounded-full border border-amber-300/60 bg-amber-100/70 px-3 py-1 text-[11px] font-semibold uppercase tracking-[0.28em] text-amber-950">
              Ferrum Observability
            </div>
            <div className="space-y-3">
              <h1 className="max-w-3xl text-4xl font-black tracking-[-0.05em] text-slate-950 sm:text-5xl">
                Real-time telemetry for every hot path in the stack.
              </h1>
              <p className="max-w-2xl text-sm leading-7 text-slate-700 sm:text-base">
                Prometheus metrics feed the backend, SSE keeps the dashboard warm, and the last sixty
                seconds stay visible so we can spot regressions without leaving the page.
              </p>
            </div>
          </div>

          <div className="grid gap-4 rounded-[1.75rem] bg-slate-950 p-5 text-slate-50 shadow-[inset_0_1px_0_rgba(255,255,255,0.12)]">
            <StatusPill label="Stream" value={streamStatus} accent={streamStatus === "live" ? "emerald" : streamStatus === "reconnecting" ? "amber" : "sky"} />
            <StatusPill
              label="Last update"
              value={lastUpdated ? new Date(lastUpdated).toLocaleTimeString() : "Waiting"}
              accent="slate"
            />
            <div className="grid grid-cols-2 gap-3">
              <SignalCard title="Req/sec" value={formatNumber(latest.req_per_sec)} hint="live throughput" />
              <SignalCard title="p99 latency" value={`${formatNumber(latest.request_duration_p99_ms)} ms`} hint="tail latency" />
              <SignalCard title="Error rate" value={`${formatPercent(latest.error_rate)}`} hint="windowed share" />
              <SignalCard title="Cache hit" value={formatPercent(latest.cache_hit_rate)} hint="proxy effectiveness" />
            </div>
          </div>
        </section>

        <section className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
          <MetricCard title="Total requests" value={formatInteger(latest.request_count)} caption="Cumulative requests observed by the agent." tone="amber" />
          <MetricCard title="Errors" value={formatInteger(latest.error_count)} caption="Cumulative failed requests across components." tone="rose" />
          <MetricCard title="Active connections" value={formatInteger(latest.active_connections)} caption="Current open traffic in flight." tone="sky" />
          <MetricCard title="Queue depth" value={formatInteger(latest.queue_depth)} caption="Queued work waiting for workers." tone="emerald" />
        </section>

        <section className="grid gap-5 xl:grid-cols-[1.45fr_1fr]">
          <Panel title="Throughput and Latency" subtitle="60 second rolling window">
            <div className="h-80">
              <ResponsiveContainerAny width="100%" height="100%">
                <AreaChartAny data={chartData}>
                  <defs>
                    <linearGradient id="reqFill" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="0%" stopColor="#f59e0b" stopOpacity={0.45} />
                      <stop offset="100%" stopColor="#f59e0b" stopOpacity={0.03} />
                    </linearGradient>
                  </defs>
                  <CartesianGridAny stroke="rgba(15,23,42,0.08)" vertical={false} />
                  <XAxisAny dataKey="time" tickLine={false} axisLine={false} minTickGap={20} />
                  <YAxisAny yAxisId="left" tickLine={false} axisLine={false} width={42} />
                  <YAxisAny yAxisId="right" orientation="right" tickLine={false} axisLine={false} width={48} />
                  <TooltipAny
                    contentStyle={{
                      borderRadius: "16px",
                      border: "1px solid rgba(15,23,42,0.08)",
                      backgroundColor: "rgba(255,255,255,0.96)",
                    }}
                  />
                  <AreaAny yAxisId="left" type="monotone" dataKey="reqPerSec" stroke="#f59e0b" fill="url(#reqFill)" strokeWidth={3} />
                  <LineAny yAxisId="right" type="monotone" dataKey="p99" stroke="#0f172a" strokeWidth={2.5} dot={false} />
                </AreaChartAny>
              </ResponsiveContainerAny>
            </div>
          </Panel>

          <Panel title="Error Pressure" subtitle="windowed percentage and connection load">
            <div className="h-80">
              <ResponsiveContainerAny width="100%" height="100%">
                <LineChartAny data={chartData}>
                  <CartesianGridAny stroke="rgba(15,23,42,0.08)" vertical={false} />
                  <XAxisAny dataKey="time" tickLine={false} axisLine={false} minTickGap={20} />
                  <YAxisAny yAxisId="left" tickLine={false} axisLine={false} width={42} />
                  <YAxisAny yAxisId="right" orientation="right" tickLine={false} axisLine={false} width={48} />
                  <TooltipAny
                    contentStyle={{
                      borderRadius: "16px",
                      border: "1px solid rgba(15,23,42,0.08)",
                      backgroundColor: "rgba(255,255,255,0.96)",
                    }}
                  />
                  <LineAny yAxisId="left" type="monotone" dataKey="errorRate" stroke="#e11d48" strokeWidth={3} dot={false} />
                  <LineAny yAxisId="right" type="monotone" dataKey="activeConnections" stroke="#0284c7" strokeWidth={2.5} dot={false} />
                </LineChartAny>
              </ResponsiveContainerAny>
            </div>
          </Panel>
        </section>

        <Panel title="Component Health" subtitle="live health registry from the observability agent">
          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            {latest.component_health.map((component) => (
              <article
                key={component.component}
                className="rounded-[1.5rem] border border-black/10 bg-white/80 p-4 shadow-[0_14px_35px_rgba(15,23,42,0.05)]"
              >
                <div className="mb-3 flex items-start justify-between gap-3">
                  <div>
                    <p className="text-xs font-semibold uppercase tracking-[0.22em] text-slate-500">
                      Component
                    </p>
                    <h3 className="mt-2 text-lg font-bold tracking-[-0.03em] text-slate-950">
                      {component.component}
                    </h3>
                  </div>
                  <span className={`rounded-full px-3 py-1 text-[11px] font-bold uppercase tracking-[0.16em] ${badgeClass(component.state)}`}>
                    {component.state}
                  </span>
                </div>
                <p className="text-sm leading-6 text-slate-700">{component.detail}</p>
                <p className="mt-4 font-mono text-xs text-slate-500">
                  Updated {new Date(component.updated_at_ms).toLocaleTimeString()}
                </p>
              </article>
            ))}
          </div>
        </Panel>
      </div>
    </main>
  );
}

function MetricCard({
  title,
  value,
  caption,
  tone,
}: {
  title: string;
  value: string;
  caption: string;
  tone: "amber" | "rose" | "sky" | "emerald";
}) {
  const toneClass = {
    amber: "from-amber-100 to-white text-amber-950",
    rose: "from-rose-100 to-white text-rose-950",
    sky: "from-sky-100 to-white text-sky-950",
    emerald: "from-emerald-100 to-white text-emerald-950",
  }[tone];

  return (
    <article className={`rounded-[1.5rem] border border-black/10 bg-gradient-to-br ${toneClass} p-5 shadow-[0_18px_40px_rgba(15,23,42,0.06)]`}>
      <p className="text-xs font-semibold uppercase tracking-[0.24em] text-black/45">{title}</p>
      <p className="mt-4 text-3xl font-black tracking-[-0.05em]">{value}</p>
      <p className="mt-3 text-sm leading-6 text-black/60">{caption}</p>
    </article>
  );
}

function Panel({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle: string;
  children: ReactNode;
}) {
  return (
    <section className="rounded-[1.75rem] border border-black/10 bg-white/72 p-5 shadow-[0_24px_60px_rgba(15,23,42,0.06)] backdrop-blur sm:p-6">
      <div className="mb-5 flex flex-col gap-2 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <h2 className="text-2xl font-black tracking-[-0.04em] text-slate-950">{title}</h2>
          <p className="text-sm text-slate-600">{subtitle}</p>
        </div>
      </div>
      {children}
    </section>
  );
}

function SignalCard({
  title,
  value,
  hint,
}: {
  title: string;
  value: string;
  hint: string;
}) {
  return (
    <div className="rounded-2xl border border-white/10 bg-white/6 p-3">
      <p className="text-[10px] font-semibold uppercase tracking-[0.22em] text-slate-400">{title}</p>
      <p className="mt-2 text-xl font-black tracking-[-0.05em] text-white">{value}</p>
      <p className="mt-1 text-xs text-slate-400">{hint}</p>
    </div>
  );
}

function StatusPill({
  label,
  value,
  accent,
}: {
  label: string;
  value: string;
  accent: "emerald" | "amber" | "sky" | "slate";
}) {
  const accentClass = {
    emerald: "border-emerald-400/30 bg-emerald-400/10 text-emerald-200",
    amber: "border-amber-400/30 bg-amber-400/10 text-amber-200",
    sky: "border-sky-400/30 bg-sky-400/10 text-sky-200",
    slate: "border-white/10 bg-white/5 text-slate-200",
  }[accent];

  return (
    <div className={`flex items-center justify-between rounded-full border px-3 py-2 text-xs ${accentClass}`}>
      <span className="font-semibold uppercase tracking-[0.22em]">{label}</span>
      <span className="font-mono">{value}</span>
    </div>
  );
}

function badgeClass(state: ComponentState) {
  switch (state) {
    case "healthy":
      return "border border-emerald-200 bg-emerald-50 text-emerald-700";
    case "degraded":
      return "border border-amber-200 bg-amber-50 text-amber-700";
    case "down":
      return "border border-rose-200 bg-rose-50 text-rose-700";
  }
}

function formatInteger(value: number) {
  return new Intl.NumberFormat("en-US", { maximumFractionDigits: 0 }).format(value);
}

function formatNumber(value: number) {
  return new Intl.NumberFormat("en-US", { maximumFractionDigits: 2 }).format(value);
}

function formatPercent(value: number) {
  return new Intl.NumberFormat("en-US", {
    style: "percent",
    maximumFractionDigits: 1,
  }).format(value);
}
