# Docker Sandbox Isolation Policy

Status: proposed — requires an explicit policy decision before implementation

## Proposal

Keep `--sandbox docker` as the current bounded workspace runner until a stronger
policy is accepted. Its supported contract is limited to:

- a reachable Docker daemon verified by `docker info`;
- the selected workspace bind-mounted read/write at `/workspace`;
- container-only temporary files not being written into the host workspace; and
- no fallback to native execution when Docker is unavailable or rejects the run.

The current mode is not a complete security sandbox. In particular, it makes no
claim about network access, Linux capabilities, privilege, root filesystem
mutability, CPU, memory, or process-count isolation. The Docker daemon and host
kernel remain trusted components, and the workspace is intentionally shared.

If stronger isolation is required, introduce it as an explicitly selected policy
profile rather than silently changing the existing `docker` defaults. A strict
profile is only a candidate until the decision below is accepted.

## Candidate strict profile

The candidate profile would preserve the `/workspace` read/write bind mount while
requesting the following Docker defaults:

| Area | Candidate default | Compatibility consequence |
| --- | --- | --- |
| Network | `--network none` | Package downloads and network-dependent shell commands fail by design. |
| Linux privileges | `--cap-drop ALL --security-opt no-new-privileges` | Commands requiring ambient capabilities or privilege fail. |
| Root filesystem | `--read-only` plus a bounded writable `/tmp` tmpfs | Tools must write durable changes under `/workspace`; temporary writes are bounded. |
| Resources | `--cpus 2 --memory 512m --pids-limit 256` | Large builds or highly parallel tools may be terminated or throttled. |

These values are not approved defaults. They must be validated against the actual
tool contract and supported Docker implementations before any CLI or configuration
surface is added. A strict profile must fail closed when Docker cannot honor the
requested policy; it must never silently fall back to native execution.

### Current-host feasibility evidence

On 2026-08-31, the candidate flag set was run once against the available Docker
Desktop Linux daemon on Windows with the `alpine` image. Docker accepted all
requested flags. The bounded probe observed:

- writes to the read-only root filesystem were denied while `/tmp` remained writable;
- `CapEff` was `0000000000000000`;
- `/proc/net/route` contained only its header, consistent with `--network none`;
- cgroup limits reported `memory.max=536870912`, `pids.max=256`, and
  `cpu.max=200000 100000` (two CPUs).

This is feasibility evidence for one Docker Desktop host only. It does not prove
workspace mounting under the strict profile, cross-platform behavior, image
provenance, daemon isolation, or compatibility with real project builds. The
proposal therefore remains proposed and the runtime defaults remain unchanged.

## Threat model and support matrix

The policy decision must explicitly state whether the goal is:

1. workspace-scoped accidental-write containment;
2. reduced host exposure from shell processes; or
3. resistance to a malicious command running under a trusted Docker daemon.

The current implementation can support the first goal only as evidence-backed
process containment. The third goal is out of scope unless the daemon, kernel,
image provenance, bind mounts, and host integration are included in the threat
model.

Before implementation, test the selected profile on Linux Docker Engine and the
Linux VM used by Docker Desktop on Windows and macOS, or explicitly narrow the
support matrix. Docker daemon absence, image absence, unsupported flags, and
runtime policy rejection must all produce a bounded `ToolError` with no native
fallback.

## Required bounded evidence

The implementation batch must add public-boundary evidence for:

- daemon and image preflight, including unavailable-daemon failure;
- `/workspace` read/write behavior and container-only temporary files;
- network denial or allowance, matching the accepted policy;
- capability/privilege and read-only filesystem behavior;
- resource-limit behavior with bounded process cleanup; and
- the exact failure result when a restriction cannot be honored.

Tests must be deterministic and runnable as an explicit Docker-enabled job when
the local or CI host has no daemon. A Docker availability or workspace-mount
probe alone cannot promote this proposal to an implemented security claim.

## Six-question admission record

1. **Layer:** Capabilities (`workspace::run_shell`) owns Docker command
   construction and process execution; CLI only selects an already accepted
   policy profile.
2. **Duplicate responsibility:** inspect `run_shell`, `ProcessSandbox`, the
   `docker info` preflight, and existing mount/temporary-file probes; do not add
   a second daemon wrapper or execution loop.
3. **Replace vs add:** preserve the current bounded `docker` behavior and add a
   strict profile only if the policy requires two useful behaviors; do not add
   flags without a threat-model decision.
4. **Net line delta:** implementation must be net-zero or name an explicit
   offset; this proposal itself has no Rust line delta.
5. **Visible surface:** a future profile changes shell behavior and possibly CLI
   or configuration semantics, but does not imply model-context, event,
   persistence, or JSON-RPC changes.
6. **Boundary evidence:** use bounded Capabilities and CLI public-path scenarios
   on every claimed platform; record unsupported hosts and fail-closed behavior.

## Decision required

Choose one before implementation:

- **Keep current contract:** retain `--sandbox docker` as bounded process
  containment and make no stronger isolation claim.
- **Accept strict profile:** approve the threat model, supported platforms,
  candidate defaults, opt-out semantics, compatibility impact, and evidence plan;
  then implement it as a separate, explicitly selected policy profile.

Until that decision is recorded, Docker runtime code remains unchanged.
