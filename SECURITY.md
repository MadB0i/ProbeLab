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
- Denial of service and resource exhaustion
- Bypassing the containment layer through a capability no shipped benchmark can currently request.
  No benchmark in the catalogue declares a `ProcessSpawnPlan` or a spawn grant, so in the desktop
  product every spawn is refused at the broker and never reaches containment. Reachability is
  part of severity here, not an afterthought.

## Design notes

- **Deny by default.** The broker authorizes nothing unless a benchmark's own policy grants it, with
  canonicalised path checks and independent read and write roots.
- **An authorization is not an execution.** Only `process.spawn` has an executor. The other four
  permissions stop the run with an explicit unsupported error — none of them has a pretend
  implementation.
- **The model cannot describe the process.** The engine rebuilds the request under review from the
  benchmark's compiled-in plan, discarding the model's framing entirely. No model can name a
  program, argument, working directory, or environment variable.
- **The child is contained before it runs.** Suspended, inside a job object with CPU/memory/process
  limits, at low mandatory integrity, in a directory it alone may write to, with an **empty**
  environment. Its integrity level is read back and path protection re-verified before it is
  resumed.
- **Fail closed.** A spawn on a non-Windows platform returns `UnsupportedPlatform` rather than
  running unconfined.

`docs/SECURITY-STATUS.md` records exactly what is enforced, what is partial, and what is deferred.

## Safe harbor

Research in good faith is protected. Stay within environments you own, do not degrade ProbeLab for
other users, and allow reasonable time for a fix before public disclosure.
