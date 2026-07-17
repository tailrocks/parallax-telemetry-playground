# Execution Stack Design

Plan 034 is feasible as one daemon mode, one child process, and one agent
simulation. It does not require real Docker spawning yet.

## Minimal Topology

The smallest useful shape is:

1. `playground daemon`: host-side CLI/daemon session in one short-lived process.
2. `playground enter`: a spawned child process that simulates the container
   boundary.
3. An in-child `invoke_agent` simulation with `execute_tool` child spans.

Real Docker would add operational cost without proving the hard part. The hard
part is context and run-id continuity across process/session boundaries, and a
spawned child with explicit environment injection proves that. Future plans can
replace the simulated container span with a Docker/mux process while preserving
the same carrier contract.

## Propagation Mechanics

The Rust telemetry library already registers a composite text-map propagator:
W3C trace context plus baggage. Plan 034 adds an environment carrier helper for
`TRACEPARENT`, `TRACESTATE`, and `BAGGAGE`.

Boundary flow:

- Host CLI to daemon: represented as a host CLI span followed by a daemon
  session span in the daemon mode. This is the local RPC/socket boundary shape
  without adding a real socket server.
- Daemon to child: the daemon injects the active daemon span context into
  `TRACEPARENT`, `TRACESTATE`, and `BAGGAGE` on the spawned child environment.
- Child start: `playground enter` extracts the environment context and sets it
  as the parent of the `container_session` span.
- Agent process: the simulated agent runs inside the container span, so
  `invoke_agent` and `execute_tool` inherit the container context.

## Invocation Stitching

The scenario script sets `CLI_INVOCATION_ID` once and also ensures
`OTEL_RESOURCE_ATTRIBUTES` contains `cli.invocation.id=<invocation-id>`. The
daemon inherits it and passes the same value to `playground enter`. The
shared telemetry library surfaces `cli.invocation.id` from
`CLI_INVOCATION_ID` as a resource attribute only for wrapped child
processes; the CLI itself stamps the id on its root spans and logs (the
jackin shape — ids never live on Resource for a natively instrumented CLI).

This keeps daemon, capsule, and agent telemetry queryable as one Parallax
invocation while still letting the orphan variant create a separate trace.

## Failure Scenario

The orphan variant runs `playground daemon --orphan`. That mode deliberately
does not inject `TRACEPARENT`, `TRACESTATE`, or `BAGGAGE` into the child. The
child still receives the same `cli.invocation.id`, so Parallax can show the
invocation contains related daemon and child/agent activity, but the child
trace is not a descendant of the daemon trace. The orphan child marks its container boundary
as a client span with `url.full=container://agent`, which lets Parallax's
existing trace evidence-gap detector flag the broken continuation as a client
span without a backend child.

The agent simulation also emits a failed `execute_tool` span with
`error.type=command_exit` so issue/error derivation has a concrete event.

## Acceptance

This scenario lets a reviewer answer:

- Did the CLI session reach the daemon boundary?
- Did the daemon spawn the container session with trace context?
- Did the agent action happen inside the container context?
- Did a tool command fail, and is that failure visible on the run story?
- Does the broken variant make the missing continuation obvious by showing
  separate daemon and child traces in the run story, with a
  `browser_without_backend` evidence gap on the orphan child trace?

Simulated today:

- container runtime
- agent process
- shell tool execution

Real today:

- process boundary via spawned child
- W3C trace context and baggage propagation through environment variables
- shared `cli.invocation.id` invocation stitching
- error span status on the failed tool event
