import {
  createFileRoute,
  useNavigate,
  useRouter,
} from "@tanstack/react-router";
import { useEffect, useState, type FormEvent } from "react";
import {
  DEMO_TENANT_ID,
  displayDate,
  errorMessageForUser,
  fetchAnalytics,
  type AnalyticsEvent,
  type AnalyticsSummary,
} from "../commerce";
import { Notice, PageFrame } from "../components";
import { runTracedStep } from "../rum";
import { APP_SCREEN_NAME, APP_WIDGET_NAME, UI_CLICK } from "../semconv";

export const Route = createFileRoute("/analytics")({
  validateSearch: (search: Record<string, unknown>) => ({
    eventName:
      typeof search["eventName"] === "string" &&
      search["eventName"].trim() !== ""
        ? search["eventName"].trim()
        : undefined,
  }),
  loaderDeps: ({ search }) => ({ eventName: search.eventName ?? "" }),
  loader: async ({ deps }) => {
    try {
      return {
        result: await fetchAnalytics({
          tenantId: DEMO_TENANT_ID,
          eventName: deps.eventName || undefined,
          limit: 50,
        }),
        error: null,
      } as const;
    } catch (error: unknown) {
      return { result: null, error: errorMessageForUser(error) } as const;
    }
  },
  component: AnalyticsPage,
});

function AnalyticsPage() {
  const navigate = useNavigate({ from: "/analytics" });
  const router = useRouter();
  const { eventName = "" } = Route.useSearch();
  const loaderData = Route.useLoaderData();
  const [eventNameInput, setEventNameInput] = useState(eventName);
  useEffect(() => setEventNameInput(eventName), [eventName]);
  const [fallbackResult, setFallbackResult] = useState(loaderData.result);
  useEffect(() => {
    if (loaderData.result !== null) {
      setFallbackResult(loaderData.result);
      return;
    }
    let active = true;
    void fetchAnalytics({
      tenantId: DEMO_TENANT_ID,
      eventName: eventName || undefined,
      limit: 50,
    }).then(
      (result) => {
        if (active) setFallbackResult(result);
      },
      () => undefined,
    );
    return () => {
      active = false;
    };
  }, [eventName, loaderData.result]);
  const state =
    fallbackResult !== null
      ? ({
          kind: "ready",
          events: fallbackResult.events,
          summary: fallbackResult.summary,
        } as const)
      : loaderData.error !== null
      ? ({ kind: "error", message: loaderData.error } as const)
        : ({ kind: "loading" } as const);

  async function applyFilter(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runTracedStep(
      UI_CLICK,
      { [APP_SCREEN_NAME]: "analytics", [APP_WIDGET_NAME]: "analytics-filter" },
      async () => {
        await navigate({
          search: (previous) => ({
            ...previous,
            eventName: eventNameInput.trim() || undefined,
          }),
        });
      },
    );
  }

  return (
    <PageFrame
      eyebrow="analytics / ClickHouse read path"
      title="See what the journey emitted."
      description="This view reads application analytics from ClickHouse through the Rust storefront GraphQL gateway. Browser RUM remains a separate OTel/Sentry signal."
      actions={
        <button
          className="button button-secondary"
          type="button"
          onClick={() => void router.invalidate()}
        >
          Refresh events
        </button>
      }
    >
      <form className="toolbar" onSubmit={(event) => void applyFilter(event)}>
        <label className="sr-only" htmlFor="event-filter">
          Event name
        </label>
        <input
          className="search-field"
          id="event-filter"
          value={eventNameInput}
          onChange={(event) => setEventNameInput(event.target.value)}
          placeholder="Filter event name, e.g. order.completed"
        />
        <button className="button button-small" type="submit">
          Apply filter
        </button>
        {eventName ? (
          <button
            className="button button-small button-quiet"
            type="button"
            onClick={() => {
              setEventNameInput("");
              void navigate({
                search: (previous) => ({ ...previous, eventName: undefined }),
              });
            }}
          >
            Clear
          </button>
        ) : null}
      </form>
      {state.kind === "error" ? (
        <Notice
          tone="error"
          title="Analytics read failed"
          action={
            <button
              className="button button-small button-secondary"
              type="button"
              onClick={() => void router.invalidate()}
            >
              Retry
            </button>
          }
        >
          {state.message}
        </Notice>
      ) : null}
      {state.kind === "ready" ? (
        <AnalyticsContent events={state.events} summary={state.summary} />
      ) : null}
    </PageFrame>
  );
}

function AnalyticsContent({
  events,
  summary,
}: Readonly<{ events: readonly AnalyticsEvent[]; summary: AnalyticsSummary }>) {
  return (
    <>
      <div className="notice notice-info">
        <div>
          <strong>
            {summary.eventName
              ? `Filtered: ${summary.eventName}`
              : "All storefront application events"}
          </strong>
          <p>Tenant {summary.tenantId} · latest 50 rows · source: ClickHouse</p>
        </div>
      </div>
      <div className="metric-grid">
        <Metric
          label="Events"
          value={summary.eventCount.toLocaleString()}
          caption="matching rows"
        />
        <Metric
          label="Customers"
          value={summary.uniqueCustomers.toLocaleString()}
          caption="unique customer ids"
        />
        <Metric
          label="First seen"
          value={
            summary.firstOccurredAt ? displayDate(summary.firstOccurredAt) : "—"
          }
          caption="event window"
        />
        <Metric
          label="Last seen"
          value={
            summary.lastOccurredAt ? displayDate(summary.lastOccurredAt) : "—"
          }
          caption="event window"
        />
      </div>
      {events.length === 0 ? (
        <div className="empty-state">
          <h2>No matching events</h2>
          <p>
            Complete a browse or checkout journey, then refresh this
            ClickHouse-backed view.
          </p>
        </div>
      ) : (
        <section className="event-list" aria-label="Analytics events">
          {events.map((event) => (
            <EventCard event={event} key={event.eventId} />
          ))}
        </section>
      )}
    </>
  );
}

function Metric({
  label,
  value,
  caption,
}: Readonly<{ label: string; value: string; caption: string }>) {
  return (
    <article className="metric-card">
      <span className="label">{label}</span>
      <strong className="metric-value">{value}</strong>
      <span className="metric-caption">{caption}</span>
    </article>
  );
}

function EventCard({ event }: Readonly<{ event: AnalyticsEvent }>) {
  return (
    <article className="event-card">
      <h3>
        <span>{event.eventName}</span>
        <span className="muted">v{event.eventVersion}</span>
      </h3>
      <div className="event-meta">
        <span>{displayDate(event.occurredAt)}</span>
        <span>·</span>
        <span>{event.source}</span>
        <span>·</span>
        <span>
          {event.entityType}:{event.entityId}
        </span>
      </div>
      <div className="event-meta" style={{ marginTop: "0.45rem" }}>
        <span className="trace-chip">event {event.eventKey}</span>
        <span className="trace-chip">
          trace {event.traceId || "not returned"}
        </span>
      </div>
      <pre className="event-properties">
        {formatProperties(event.properties)}
      </pre>
    </article>
  );
}

function formatProperties(properties: string): string {
  try {
    const parsed: unknown = JSON.parse(properties);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return properties;
  }
}
