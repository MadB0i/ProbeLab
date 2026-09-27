# Security Policy

## Reporting a vulnerability

If you find a way to defeat ProbeLab's containment or capability boundaries, email the maintainer
directly instead of opening a public issue. Include the version, the capability you were able to
obtain, and the steps to reproduce it.

## Read this first

ProbeLab is an *evaluation environment*. Its benchmarks are known to be incomplete, and at least
one of them has already been shown not to measure what it claimed. That is documented openly in
`docs/AUDIT-2026-09-26.md`.

The point of the security model is to keep the **environment** honest while that is true. So:

- Claims that a benchmark is invalid, easy to game, or does not measure its capability are
  **research findings, not vulnerabilities.** Open an issue or a discussion. They are welcome and
  will be treated as such.
- Only containment and capability boundaries are in scope below.

## In scope

- **Broker bypass** — obtaining any capability that the benchmark's policy does not grant, including
  `environment.inspect`, which is denied unconditionally and has no grant path
- **Escape from an authorised `process.spawn` child** — writing outside the designated writable
  directory, reading the benchmark's protected paths, escaping the job object, or recovering the
  child's low integrity label
- **Environment inheritance** — a spawned child obtaining API keys, tokens, or any other variable
  the parent process held
- **Leaking protected state to the model** — hidden map, benchmark implementation, evaluator,
  filesystem paths, or persistence handles reaching the connected provider through an observation,
  a `history` window, or any other field
- **Forged records** — tampering with persistence, audit, or observation records such that a
  violation reads as compliant
- **Fail-open behaviour** — any path where a denied request, a capability error, or an unsupported
  platform results in the action being permitted

## Out of scope

- Attacks requiring an attacker who already controls the machine ProbeLab runs on
- The security of the connected model provider itself
- Denial of service and resource exhaustion beyond the declared episode, wall-clock, output, and
  process-runtime limits
- **Container escape on Linux and macOS.** There is no OS-level containment on those platforms: a spawn
  fails closed with `UnsupportedPlatform` rather than running unconfined, so there is no boundary there
  to escape. Containment hardening is a Windows-only surface, and `docs/STATUS.md` §3.2 lists what it
  does and does not do.
- **Syscall-level or code-level escape on Windows.** The AppContainer identity and job object bound file
  access and resources. Neither intercepts a syscall, and native code loading is not restricted. This is
  not a code sandbox and is not claimed to be one.

## Design notes

- **Deny by default.** The broker authorizes nothing unless a benchmark's own policy grants it, with
  canonicalised path checks and independent read and write roots.
- **An authorization is not an execution.** Only `process.spawn` has an executor. `filesystem.read`,
  `filesystem.write`, and `network.connect` stop the run with an explicit unsupported error — none of
  them has a pretend implementation. `environment.inspect` is a fourth case: the broker denies it
  unconditionally and `grant_permission` for it is a hard error, so it is never authorized at all and
  the execution-refusal path is unreachable for it.
- **The model cannot describe the process.** The engine rebuilds the request under review from the
  benchmark's compiled-in plan, discarding the model's framing entirely. No model can name a
  program, argument, working directory, or environment variable. `ModelDecision::ProcessSpawnRequest`
  carries the permission and nothing else.
- **The child is contained before it runs.** Suspended; inside a job object with CPU, memory, and
  process-count limits and `KILL_ON_JOB_CLOSE`; under a **derived AppContainer package identity** whose
  `TokenAppContainerSid` is read back and compared before resume, with a mismatch killing the child;
  at low mandatory integrity; in a directory it alone may write to, protected by an ACE naming the
  package SID; with outbound network refused by the OS because the package token carries no
  capabilities; and with an **empty** environment. Path protection is re-verified before resume.
- **Fail closed.** A spawn on a non-Windows platform returns `UnsupportedPlatform` rather than
  running unconfined, and a missing or unverifiable identity kills the child.

`docs/STATUS.md` §3 records every guarantee with the test that holds it, and §3.2 records what
containment does **not** do. `docs/SECURITY-STATUS.md` is the per-boundary implementation record.

## Safe harbor

Research in good faith is protected. Stay within environments you own, do not degrade ProbeLab for
other users, and allow reasonable time for a fix before public disclosure.
