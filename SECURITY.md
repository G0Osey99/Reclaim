# Security Policy

## Reporting a vulnerability

Please report security issues **privately**, not in a public issue:

- Use GitHub's private vulnerability reporting: the repository's **Security** tab →
  **Report a vulnerability** (GitHub Security Advisories).

Include the version (`reclaim --version` or the app's About box), your OS, and
steps to reproduce. We aim to acknowledge within a few days and to coordinate a
fix and disclosure timeline with you.

Please do not open a public issue or PR for a suspected vulnerability until a fix
is released.

## What we treat as a security bug

Reclaim's core safety property is that **it never writes to a source**. The
highest-severity issues are therefore:

- **Any write to a scanned source device** (a P0 — see the read-only guarantees in
  the [README](README.md#safety-promises) and `docs/plan/09` §5).
- **Data loss during recovery** (corrupting or overwriting the destination, or the
  same-disk refusal failing to trigger).
- **Privilege issues in the macOS helper** (`com.reclaim.helper`): the privileged
  helper exposes only read-only device-open, SMART query, and user-supplied APFS
  unlock over XPC, and both ends pin a code-signing requirement. A path that lets
  an unauthorized caller drive it, or that widens the helper's surface, is in scope.
- **Parser memory-safety / panics on crafted on-disk data.** Reclaim's whole job
  is parsing hostile bytes; every parser is fuzzed and must not panic. A crash on
  malformed input is a bug (report the sample); a memory-safety violation is a
  security bug.
- **Anything that would exfiltrate user data.** Reclaim has no telemetry and no
  network features in the core/CLI; a path that sends recovered content, file
  names, or session data anywhere is in scope.

## Out of scope

- The inability to recover data that has been overwritten or TRIM'd (a physics
  limit, not a bug — see the chances-by-media table in the README).
- Findings that require already having root/physical access equivalent to what the
  user themselves grants (e.g. reading a device the user explicitly authorized).

## Supported versions

Round 1 supports the latest **1.x** release. Security fixes land on the newest
minor and are released as a patch version.
