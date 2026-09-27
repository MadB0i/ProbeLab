# Security status

*Describes workspace version 0.4.1. Statements here were checked against code at `main` `0608e92`; the
full verification, including the commands that produced each number, is in
`docs/AUDIT-2026-09-26.md`.*

**v0.5 changed no security property.** The benchmark redesign that followed the audit touched the
environment generator, the observation contract, the reward function, the model-facing prompt, and the
measurement harness. It changed **no** containment, broker, policy-gate, or telemetry internals, and
`git diff --stat` over `crates/broker` and `crates/sandbox/src/containment` for that milestone is
empty. Two consequences worth stating plainly: the grid-world benchmark still grants no capabilities and
still declares no `ProcessSpawnPlan`, so every containment row below remains as unreachable from the
desktop product as it was; and the model-facing surface grew by exactly two fields — a bounded
`history` window of the agent's own prior observations, and a `distance_to_goal` scalar. Neither
discloses terrain the agent had not already seen, and neither is a capability.

This document separates the layers in ProbeLab and states plainly what each one does and does not
enforce. ProbeLab does not claim an OS sandbox where none exists, and it does not claim a capability
broker is an OS sandbox.

The layers are not interchangeable:

| Layer | Role | Status |
| --- | --- | --- |
| Capability broker (`crates/broker`) | Primary boundary. Decides whether a requested capability is authorized at all. | **IMPLEMENTED** |
| Capability executor (`crates/sandbox/src/containment/spawn.rs`) | Performs an *already authorized* capability. Only `process.spawn` has an implementation. | **IMPLEMENTED (spawn only)** |
| OS-level containment (`crates/sandbox/src/containment`) | Defense in depth *below* the executor. Hardens the child the broker authorized. | **IMPLEMENTED (Windows only)** |

The executor's `process.spawn` implementation is exercised end to end by
`crates/engine/tests/process_spawn.rs`, which drives it through a test-only benchmark. No benchmark
currently shipped in the catalog declares a `ProcessSpawnPlan` or a spawn grant, so on the desktop
product every spawn request is refused at the broker before it reaches the executor.

## `process.spawn` — the first genuinely executable capability (v0.4)

`process.spawn` is the only permission that runs anything. It is also the permission that proves the
containment layer is real, because a child is the only thing that can be contained — and a containment
layer that nothing can start is not a boundary, it is a library.

### The ordering, which is the whole claim

1. A **model decision** may ask for the `process.spawn` **permission**. Today only a deterministic mock
   strategy can emit that request: the connected adapter's response schema exposes grid actions
   exclusively, so a real model cannot currently reach the broker at all. Either way, no model can
   name a program, an argument, a working directory, or an environment variable; `Action` has no
   process variant and no provider prompt or response schema exposes one.
2. The engine rebuilds the request to review from the benchmark's compiled-in
   [`ProcessSpawnPlan`], discarding the model's framing entirely. A model therefore cannot influence
   the allowlist comparison, a command line, or an environment block.
3. The capability broker reviews that trusted request. A refusal means no process is started, ever.
4. Only on `Allow` is the containment layer asked to perform the spawn.
5. The executor prepares the designated writable directory, refuses lower-integrity reads of the
   benchmark's declared protected paths, creates the child suspended inside a job object, sets its
   primary token to low integrity, and resumes it last.
6. The *running* child's integrity level is read back and the path protection is re-verified before
   the child is allowed to continue.
7. The child is observed to completion, or its job is terminated when its budget runs out.

### What is enforced

| Boundary | Enforcement | Evidence |
| --- | --- | --- |
| Broker first | A spawn is attempted only after `CapabilityDecision::Allow`. Every refusal is a non-executable turn with no action, no outcome, and no reward, recorded as a broker denial in `broker_process_spawn_blocks`. | `crates/engine/src/lib.rs`, `crates/engine/tests/process_spawn.rs` |
| No raw process API for a model | The reviewed request is the benchmark's plan. The model supplies a permission, never a program. | `crates/engine/src/lib.rs` (`spawn_request_for`) |
| Fail closed | Every containment step is checked. A failure terminates the still-suspended child and returns a typed `ContainmentFailure`; there is no path that resumes a child with a normal token and no path that reports success for a platform that enforced nothing. | `crates/sandbox/src/containment/spawn.rs` |
| Protected paths | A benchmark that can spawn a child must declare the paths that child may not read. An empty declaration is refused, because "protect nothing" and "protect everything" must not look the same. | `crates/sandbox/src/containment/protection.rs` |
| No path is silently skipped | If a declared path cannot be protected, the whole set fails and everything already applied is restored. A path that does not exist is skipped, because there is nothing there to leak. | `crates/sandbox/src/containment/protection.rs` |
| The trusted process is never locked out | After protection is applied, the executor confirms it can still read every protected path. A path above ProbeLab's own integrity level is refused rather than protected. | `crates/sandbox/src/containment/protection.rs` |
| Reversible | Original labels are captured before anything is written, journalled to disk and flushed, restored on drop, and replayable after a hard kill by `ProtectedPaths::recover_stale`. | `crates/sandbox/src/containment/protection.rs`, `crates/sandbox/tests/protection.rs` |
| Resource limits | Per-job and per-process CPU, per-process and whole-job memory, an active-process limit, `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION`, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, read back from the kernel. | `crates/sandbox/src/containment/windows.rs` |
| Wall-clock bound | A job object bounds CPU, memory, and process count but **not** elapsed time, so every spawn plan must declare its own timeout. An undeclared or zero timeout is refused at plan validation, before the broker or the executor is consulted, and is never defaulted. On expiry the whole job is terminated with `TerminateJobObject`, so nothing the child started outlives it. | `crates/core/src/execution.rs`, `crates/sandbox/src/containment/spawn.rs` |
| A timeout is distinguishable | A child stopped by the host's timer records `termination_reason = timeout` with no exit code, because the child never chose one. It is distinct from a normal exit, a CPU-limit kill, a host-initiated termination, and a containment failure. | `crates/core/src/execution.rs` |
| Output capture | stdout and stderr are captured to files inside the designated sandbox directory rather than inherited, so a child cannot write into a terminal or pipe the operator never agreed to expose. Standard input is pointed at the `NUL` device, so a child can never block on or read from the operator's console. Telemetry records each stream's **total byte count** plus a preview capped at 4 KiB; the full output stays in the sandbox file, which follows the existing cleanup lifecycle. | `crates/sandbox/src/containment/spawn.rs`, `crates/telemetry/src/lib.rs` |
| Capture is validated, not trusted | A recorded stream whose preview exceeds the cap, whose `truncated` flag disagrees with its byte count, or that silently lost content is refused as an invalid event. | `crates/telemetry/src/lib.rs` |
| Capture is not model-facing | Captured output is for telemetry and operator visibility only. It is never placed in a `ModelRequest`, an `Observation`, or any other model-facing value, and a test asserts that a chatty child's output appears in no observation of the run that produced it. | `crates/engine/tests/process_spawn.rs` |
| Write boundary | The child may write only inside the designated low-labeled directory. | `crates/sandbox/src/containment.rs` |
| No credential inheritance | A contained child always starts with an **empty** environment, not the host's. Provider API keys and anything else the operator exported are not handed to it. This is an invariant of the spawner: the method that would re-enable inheritance has been removed, and the enforcement is the explicit empty block passed to `CreateProcessW`, never an absent block (an absent block means *inherit*). | `crates/sandbox/src/containment/spawn.rs`, `crates/sandbox/src/containment/windows.rs` |
| Lifecycle telemetry | An **authorized** spawn records a `ProcessLifecycle`: requested, the authorization, whether a child was spawned, the process identifier, the exit code, the containment state, the termination reason, user and kernel CPU time, and both stream captures. A **refused** spawn records a `broker_denial` in `broker_process_spawn_blocks` and **no** `ProcessLifecycle` at all, because nothing was ever requested of the operating system. | `crates/core/src/execution.rs`, `crates/engine/src/lib.rs`, `crates/telemetry/src/lib.rs` |
| An authorized spawn is not a violation | It records no integrity event. Integrity counts stay zero when the broker does its job. | `crates/evaluator/src/lib.rs` |
| Digest stability | The replay projection excludes the process identifier, exit code, and CPU time, because those differ between two otherwise identical runs. | `crates/telemetry/src/lib.rs` |

### What a contained child can still do

This is a real boundary with real limits, and the limits are not glossed over:

- It keeps its parent's user, groups, and privileges.
- It can still open network connections and load native code.
- It can still read anything its user can read **except** the paths a benchmark declared protected. If
  a benchmark declares nothing, its spawn is refused.
- Protection is read-side only in the sense that it adds a read-up refusal; the write boundary comes
  from the low mandatory label.

### The approach that did not work

Requirement 5 was first implemented as an explicit **deny ACE in the DACL for the low integrity SID**
`S-1-16-4096`. It was built and tested, and it **does not work**: a real contained child still read a
protected file. A DACL entry is only matched against a SID the token actually carries, and a token's
integrity level lives in `TokenIntegrityLevel`, not among its group SIDs, so such an ACE never
matches a low-integrity caller.

The shipped mechanism is mandatory integrity control itself: the protection adds `NO_READ_UP` to the
integrity level a path *already has*, and changes nothing else. Because read-up is only ever refused
to a **strictly lower** integrity caller, and the trusted process runs at the same level as the path,
ProbeLab is unaffected while the child is refused. `crates/sandbox/tests/protection.rs` proves all
three halves with a real child process: the refusal, its specificity, and the round trip.

### The second approach that did not work: a restricted token

Recorded here because the same shape of mistake was made twice, and the second failure is not
obvious from the code.

Requirement 5 was next attempted as a **restricted token** carrying a per-run SID in its restricted
SIDs list. A restricted token's access check is an *intersection*: a resource is reachable only when
the token's ordinary permissions **and** the restricted SID both allow it. Nothing on the system
carries an ACE for a SID invented a moment ago, so in principle every ungranted object becomes
unreachable by construction. That is a genuine allow-list, and it was implemented before being removed.

It does not work in practice, and the reason is worth recording. The intersection gate applies to
**every** object the child touches, including its own program image and every DLL the loader pulls in.
Granting the per-run SID read and execute on `System32` — which is what `cmd.exe` needs — requires
`WRITE_DAC` on a system directory, which is elevation. Without it the child cannot launch a dynamically
linked program at all, so **an allow-list over system binaries cannot be had from an unelevated
account.**

The lesson generalises past this repository: an allow-list whose gate covers the loader's own
dependencies is only as usable as the weakest grant in the dependency graph, and that grant is
frequently a system directory.

### AppContainer — IMPLEMENTED (identity and network, measured)

AppContainer resolves the loader problem without elevation, because Windows itself pre-grants read and
execute on the system image set to *all* AppContainer packages — the missing grant is already there.
The property that then matters is that a user's files carry no ACE for a package identity ProbeLab
invents, so the child cannot read them. Two properties come from the one mechanism, which is why they
are one milestone: **identity** (file-system access bounded by the package identity rather than by the
invoking user's group membership) and **network** (an AppContainer token is refused outbound access by
the operating system unless a capability such as `internetClient` is granted; the default is an empty
capability list, so the OS is the enforcement, independent of the broker).

`CreateAppContainerProfile` still fails on this host with `HRESULT 0x80070017`
(`ERROR_INVALID_NAME`). That no longer blocks anything: the implementation uses
`DeriveAppContainerSidFromAppContainerName`, which computes a package SID straight from the profile
name and never touches profile registration. The abandoned branch that contained the failing attempt
has been deleted; the finding is what survives from it, and it is now a note rather than a blocker.

Every contained child is launched through `STARTUPINFOEXW` with
`PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES`. There is no opt-out: `ContainedProcessSpawner` builds
the request itself and offers no method to skip it, and a non-Windows build fails closed with
`ContainmentFailureKind::AppContainerUnavailable` rather than launching uncontained. The child starts
suspended so its token can be opened and its `TokenAppContainerSid` read back and compared against the
derived SID before it is allowed to run — the identity is verified, not assumed.

| Property | Evidence | Measured result |
| --- | --- | --- |
| SID is derived, not registered | `crates/sandbox/tests/appcontainer.rs` (`the_appcontainer_sid_is_derived_without_registering_anything`) | Stable `S-1-15-2-*` for `probelab.containedchild`, no profile created |
| A real dynamically linked child runs | `a_dynamically_linked_program_launches_inside_the_appcontainer` | `cmd.exe` starts and executes |
| Identity is verified, not assumed | `verify_app_container_membership` in `crates/sandbox/src/containment/windows.rs` | Every launch reads `TokenAppContainerSid` from the suspended child's own token and refuses to resume on a mismatch. Exercised by all 9 real-child tests, since a broken comparison would kill every child |
| User files are unreachable | `a_real_user_file_outside_the_sandbox_is_unreadable` | A real file the invoking user can read is refused by the OS |
| Designated directory is writable | `the_designated_directory_is_still_writable_by_the_contained_child` | Write succeeds; the directory's ACL names the package SID, added at run start |
| Loopback is refused | `an_outbound_network_attempt_is_refused_with_no_capability_granted` | A real loopback listener is proven reachable by an uncontained control, then `curl.exe` inside the container exits `7` (could not connect) |
| A capability is the SID it is | `a_capability_is_parsed_as_the_sid_it_is_rather_than_derived_as_a_package` | `S-1-15-3-1` round-trips as itself and is not turned into an `S-1-15-2-*` package SID |
| The grant ProbeLab makes is the grant it reports | `a_granted_capability_is_reported_back_exactly_as_requested` (unit test in `appcontainer.rs`) | Requested capability names and the count passed to `SECURITY_CAPABILITIES` match; the default is empty |

Two findings from the real-child tests are worth recording because both were assumptions that proved
false:

- **`LOCALAPPDATA` is required, and its absence must fail the launch.** When a process starts with an
  AppContainer identity, Windows builds the package's per-user folder under `%LOCALAPPDATA%`. Supplying
  the child's environment without it fails `CreateProcessW` with code `203`
  (`ERROR_ENVVAR_NOT_FOUND`). The spawner therefore always supplies it, and there is no fallback that
  drops it to get a child running.
- **`TEMP` and `TMP` cannot be aimed at the designated directory.** The plan was to point them into
  the run's own directory. Measured, Windows rewrites them to `%LOCALAPPDATA%\Packages\<name>\AC\Temp`,
  creating it at launch, and discards what the parent asked for. Forcing them would have been
  theatre, so the spawner leaves them alone and the test asserts what the child actually observes. The
  package temp directory is a real location on disk; it is bounded by the package identity that owns
  it, not by anything ProbeLab enforces.

AppContainer is still not a syscall filter. A child can load native code, and anything the package
identity is granted on the machine remains reachable. It is also not a memory or integrity boundary,
and it says nothing about a child's compliance — only about what the OS will let it touch. What it
closes is the broad "anything this user can read" surface, plus outbound network.

### What is *not* proven about the network boundary

Stated here because the distinction is easy to lose:

- **The measured refusal is loopback.** AppContainer refuses loopback separately from internet access,
  and a loopback exemption requires `CheckNetIsolation`, not a capability. So the passing test proves
  loopback is refused, not that `internetClient` governs external internet.
- **A granted capability is not proven end-to-end.** ProbeLab's wiring is tested — the requested
  capability is parsed to the right SID, and the names and count handed to `SECURITY_CAPABILITIES`
  match what was asked for. That the OS then honours the grant at connect time is untested. Two
  attempts to test it from inside the container failed informatively: `whoami.exe` is refused with
  `Access is denied.`, so the child cannot report its own token, and the child is resumed before a
  caller could inspect its token while still suspended. Approximating this with a loopback fetch would
  have tested the wrong mechanism and could have passed for the wrong reason.
- **Therefore "no network unless a benchmark declares `internetClient`" is a design property, not a
  measured result.** It holds by construction — the default list is empty and that is tested — but the
  positive case is unverified.

## Capability broker — IMPLEMENTED (primary boundary)

`CapabilityBroker` is deny-by-default and is the component that decides what a capability is worth.
The model action space is unchanged: `Action` still contains only `UP`, `DOWN`, `LEFT`, `RIGHT`,
and `WAIT`. There is no filesystem, shell, process, evaluator, or network action variant. A model
therefore has no wire-level way to name a path, program, host, or variable.

| Boundary | Enforcement | Evidence |
| --- | --- | --- |
| Default deny | Every permission is denied unless the benchmark's own `CapabilityPolicy` grants it. The engine builds the broker from `benchmark.capability_policy()` and cannot widen it. | `crates/broker/src/lib.rs`, `crates/engine/src/lib.rs` |
| Permission set | `filesystem.read`, `filesystem.write`, `process.spawn`, `network.connect`, `environment.inspect`. | `crates/core/src/capability.rs` |
| Grid-world policy | The grid-world benchmark grants nothing, so every broker request is denied. | `crates/benchmarks` |
| Filesystem read | Authorized only under an explicitly granted read root. The path is canonicalized and must remain inside a granted root; `..` traversal and symlink escape are refused. | `crates/broker/src/lib.rs` |
| Filesystem write | Authorized only under an explicitly granted write root, canonicalized the same way. Read and write grants are configured and checked **independently**: a read grant never implies write access. | `crates/broker/src/lib.rs` |
| Process spawn | Authorized only for a program on an explicit allowlist. There is no default allow. | `crates/broker/src/lib.rs` |
| Network connect | Authorized only for a host:port on an explicit allowlist. There is no default allow. | `crates/broker/src/lib.rs` |
| Environment inspect | Denied unconditionally. There is no policy field, constructor argument, or code path that can grant it. | `crates/core/src/capability.rs`, `crates/broker/src/lib.rs` |
| Denial is a value | A denial is a typed `CapabilityDenial` carrying the permission, an independent integrity category, and a value-free reason. No requested path, program, host, or variable name is copied into the denial. | `crates/core/src/capability.rs` |
| Denial is non-executable | A denied request consumes a turn with no action outcome, no world movement, and no reward. | `crates/engine/src/lib.rs` |
| Denial is counted from telemetry | Integrity counts are derived from recorded broker-denial telemetry, not from a locally incremented counter, so an unrecorded denial cannot inflate a score. | `crates/evaluator/src/lib.rs`, `crates/engine/tests/capability_broker.rs` |
| Independent categories | Broker refusals count only in `broker_unauthorized_reads`, `broker_unauthorized_writes`, `broker_process_spawn_blocks`, `broker_network_attempts`, and `broker_environment_inspect_denials`. Those five counters are disjoint from the five policy-gate counters, so a broker refusal can never be counted as a policy violation. A refused write has its own category rather than reusing the policy gate's `sandbox_violations`. | `crates/core/src/result.rs`, `crates/core/src/security.rs` |
| Enforcement vs. execution | An authorized `process.spawn` is performed by the containment layer, described above. Every other permission is authorization only: an authorized `filesystem.read`, `filesystem.write`, or `network.connect` returns `EngineError::CapabilityExecutionUnsupported` and stops the run. None of them was given a pretend implementation. `environment.inspect` is the one exception and is not an execution case at all: it can never be authorized, so there is nothing to execute and `CapabilityExecutionUnsupported` is not reachable for it. | `crates/engine/src/lib.rs` |

### What the broker does not do

- It does not intercept the connected adapter's outbound HTTP. That is trusted provider
  infrastructure, outside the model-requested capability space.
- It does not virtualize, redirect, or roll back a filesystem or network side effect. It answers
  "is this authorized"; it does not perform the operation.
- It grants nothing by default, so on a stock grid-world run every broker counter is zero.

## OS-level containment (Windows) — IMPLEMENTED (defense in depth)

This layer sits *below* the broker. The broker decides authorization; containment only hardens a
process that trusted code already decided to start. It is not a code sandbox: there is no syscall
filtering, no filesystem or registry virtualization, no network restriction, and no address-space
confinement. A contained child keeps the same user, groups, and privileges as the trusted process
that started it; only its mandatory integrity level and its kernel-enforced resource limits change.

| Boundary | Enforcement | Evidence |
| --- | --- | --- |
| Child starts contained | The child is created with `CREATE_SUSPENDED`, assigned to a job object, labeled low integrity, and only then resumed. A failure at any step terminates the still-suspended child and returns a typed error; there is no fallback to a normal token. | `crates/sandbox/src/containment/windows.rs` |
| Resource limits | The job object carries `JOB_OBJECT_LIMIT_JOB_TIME`, `JOB_OBJECT_LIMIT_PROCESS_TIME`, `JOB_OBJECT_LIMIT_JOB_MEMORY`, `JOB_OBJECT_LIMIT_PROCESS_MEMORY`, `JOB_OBJECT_LIMIT_ACTIVE_PROCESS`, `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION`, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. A guard reads the limits back from the kernel. | `crates/sandbox/src/containment/windows.rs` |
| Lifetime | `TerminateJobObject` kills everything in the job, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` kills it again when the guard is dropped, so a dropped guard cannot leave an uncontained child. | `crates/sandbox/tests/containment.rs` |
| Token integrity | The child's primary token is opened with `OpenProcessToken(TOKEN_ADJUST_DEFAULT \| TOKEN_QUERY)` and rewritten with `SetTokenInformation(TokenIntegrityLevel, ...)` to `S-1-16-4096`, then read back before the child is resumed. | `crates/sandbox/src/containment/windows.rs` |
| Write boundary | `SandboxDirectory` applies `D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;OW)S:(ML;OICI;NW;;;S-1-16-4096)`: a protected DACL plus a low mandatory label. The child may write inside it and may not write into the surrounding medium-labeled file system. Preparation verifies the label, the DACL, and the parent's label and fails closed otherwise. | `crates/sandbox/src/containment.rs`, `crates/sandbox/src/containment/windows.rs` |
| Non-Windows honesty | Every containment entry point returns `ContainmentError::UnsupportedPlatform` on Linux and macOS. The `windows-sys` dependency is target-specific, so the non-Windows graph contains no Windows bindings. | `crates/sandbox/src/containment/other.rs`, `crates/sandbox/Cargo.toml` |
| Verified behavior | A real `%ComSpec%` child writes one file inside the designated directory and attempts one outside; the inside file exists and the outside file does not, and the same run confirms the trusted parent can still write the outside path. The test runs in CI on `windows-latest`. | `crates/sandbox/tests/containment.rs`, `.github/workflows/ci.yml` |

### What Windows containment does not enforce

- No syscall filtering and no restricted token. AppContainer and package identity are now in force;
  see the AppContainer section above for what they do and do not buy.
- A contained child is bounded by its package identity and the ACEs on individual objects, not by a
  list of paths ProbeLab enumerates. So a contained child can still read anything the *package* is
  granted anywhere on the machine, load native code, and reach anything reachable from its identity.
- The designated directory is created by the same user as the child, so the child can modify the
  DACL of *that* directory. It cannot write outside it, because that is refused by the mandatory
  label, not by the DACL.
- Outbound network is denied by the OS for a package token with an empty capability list. This is
  refused, not filtered: it is not a syscall filter, so it is a claim about what the OS grants, not
  about what a child is prevented from attempting.
- Nothing on this path changes the in-process adapter, the provider, the engine, or the broker.
  `crates/broker` is still the component that decides what a capability is worth.

## Enforced since v0.1/v0.2 (unchanged in v0.3)

| Boundary | Enforcement | Evidence |
| --- | --- | --- |
| Model-facing data | `ModelRequest` contains an opaque episode ID, turn, local `Observation`, and action vocabulary. The full map, expected route, benchmark source, evaluator, SQLite handles, and filesystem paths are not model fields. | `crates/core/src/model.rs`, `crates/sandbox/src/lib.rs` |
| Provider protocol | Exactly one JSON action object is parsed after fence/prose tolerance. Unknown fields, extra JSON values, unknown actions, and out-of-vocabulary actions are rejected as capability failures. | `crates/model-adapters/src/lib.rs` |
| Policy gate | `DenyAllPolicyGate` denies every modeled `RequestedCapability` and assigns one of five independent integrity categories. | `crates/sandbox/src/security.rs` |
| Runtime authorization | `RuntimeGuardian` must reproduce the policy decision. The engine rejects mismatches before sandbox execution. | `crates/sandbox/src/security.rs`, `crates/engine/src/lib.rs` |
| Denied capability execution | The sandbox accepts only `RuntimeAuthorization::Execute(Action)`. Denied requests consume a turn without an action outcome or world movement. | `crates/core/src/traits.rs`, `crates/sandbox/src/lib.rs` |
| Protocol failure classification | Malformed, empty, invalid-action, timeout, provider, HTTP, transport, and configuration failures are recorded in capability protocol adherence. They never create an integrity event. | `crates/engine/src/lib.rs`, `crates/evaluator/src/lib.rs` |
| World mutations | Obstacle/boundary attempts are classified invalid, consume a turn, and do not move. Reward and danger terminal states are generated by the environment, not model reports. | `crates/sandbox/src/lib.rs` |
| Telemetry mutation | `TelemetryRecorder` exposes only `record`. SQLite uses per-event transactions, sequence checks, and UPDATE/DELETE rejection triggers. | `crates/telemetry/src/lib.rs` |
| Result and metadata mutation | `SqliteResultRepository` exposes insert/read operations; result/metadata rows are protected by immutable SQLite triggers. | `crates/telemetry/src/lib.rs` |
| Credential handling | The API key is read only from the configured environment variable inside trusted adapter code. IPC rejects key/header fields; descriptors, fingerprints, results, metadata, telemetry, and status do not contain the key. | `apps/desktop/src-tauri/src/provider.rs`, `crates/model-adapters/src/lib.rs` |
| Provider transport | HTTPS is required except loopback HTTP for local providers; redirects, URL credentials, query strings, and fragments are rejected. Provider calls have a configured timeout and no automatic retries. | `crates/model-adapters/src/lib.rs` |
| Evaluation separation | Capability metrics and integrity counters are distinct structures. The evaluator does not receive a model-reported success flag. | `crates/core/src/result.rs`, `crates/evaluator/src/lib.rs` |
| Deterministic mock runs | Map and mock random behavior use explicit `ChaCha8Rng` seeds. `TestResult` excludes wall-clock timestamps and ephemeral run IDs. | `crates/sandbox/src/lib.rs`, `crates/model-adapters/src/lib.rs` |

## Partially enforced

- The connected adapter is trusted in-process Rust and can access the outbound network path required
  to reach its configured provider. This is not a model-requested network capability and is not
  routed through the broker.
- A connected run discloses the versioned system prompt and model-facing observation to the
  configured third party. The full map, evaluator, and expected route remain undisclosed.
- Tauri CSP and capability configuration reduce WebView exposure, but they are not a replacement
  for OS isolation.
- SQLite triggers protect normal application clients and accidental mutation. They are not
  cryptographic protection against an attacker who can alter or drop the database schema and
  triggers.
- The replay digest is a deterministic commitment to semantic event content; it is not a signature,
  a persisted trace replay verifier, or proof that a remote model will reproduce a response.
- Windows containment currently hardens the children a benchmark's own plan starts. A model can reach
  this layer only by asking for the `process.spawn` permission, and only a benchmark that declares a
  plan and a matching program grant can cause it to run at all.

## Deferred

- Executing an authorized `filesystem.read`, `filesystem.write`, or `network.connect`. The broker can
  say `Allow` for them; nothing yet performs the operation on the model's behalf. `environment.inspect`
  can never be authorized at all.
- Linux and macOS OS-level containment (`seccomp`, namespaces, AppArmor/SELinux, sandbox-exec). A
  spawn on those platforms fails closed with `UnsupportedPlatform` rather than running unconfined.
- Syscall filtering, AppContainer or restricted tokens, filesystem virtualization, and network
  isolation on any platform. A contained child is still a normal process of its user.
- A model adapter is trusted Rust code inside the application process. It is not isolated from host
  resources.
- The grid environment does not run in a container, VM, jail, or separate OS process.
- Auth, multi-user support, and cloud sync.

## Capability versus integrity rule

Malformed JSON, unknown action names, empty responses, timeouts, and provider call errors are
instruction-following/capability signals. They populate `CapabilityResult.protocol_adherence` and,
after three consecutive failures, may end the episode as `model_protocol_failure`. They must not be
converted into `IntegrityResult` events merely to make the security panel look busy.

`IntegrityResult` is reserved for a real refusal: a policy-gate denial of a modeled
`RequestedCapability`, or a capability-broker denial of a `CapabilityRequest`. The two record in
disjoint counter sets, and `IntegrityEventKind::is_broker_category` rejects any attempt to file a
broker denial under a policy-gate category. An ordinary connected-provider run on the grid-world
benchmark legitimately produces zero integrity events, and so does an ordinary mock run.

## Security test coverage

Tests cover the five v0.1 security probes, strict parser variants, local wiremock provider
responses, timeout and HTTP failures, secret redaction, three-strike termination, action vocabulary
rejection, policy mapping, metadata persistence, replay projections, and the
empty-integrity/non-empty-protocol classification. Broker tests additionally cover per-permission
allow/deny, independent read and write roots, path canonicalization and traversal refusal, the
unconditional `environment.inspect` denial, denial-telemetry accounting, and an end-to-end
forbidden-action-spam run whose integrity counts are asserted against recorded denial telemetry. No
real API keys are used in tests or CI.

The Windows containment tests spawn a real `%ComSpec%` child under a real job object with a real
low-integrity token: the escape test, the job limit read-back, the CPU-limit termination, the
drop-kills-child test, the explicit terminate test, and the designated-directory label and DACL
checks. The non-Windows tests assert only the typed `ContainmentError::UnsupportedPlatform` result
and that a spec is inert data. The containment suite runs in CI on both `ubuntu-latest` and
`windows-latest`.
