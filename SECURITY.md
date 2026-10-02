# Security policy

OpenReadout parses untrusted binary files. A malformed file must never cause anything worse than a clean error and a non-zero exit code. A crash, a hang, unbounded memory use, a write to an unexpected place or any network connection is a security bug.

## Supported versions

OpenReadout is before 1.0. Only the latest release receives security fixes.

## Reporting a vulnerability

Email **openreadout@gmail.com**. Please do not open a public issue.

Include:

- the OpenReadout version (`openreadout --version`) and your platform;
- the command you ran;
- a minimal file that reproduces the problem, if you can share one.

Never send files that contain patient-identifiable or other personal data. If the file cannot be shared, describe how it was made.

## What to expect

- An acknowledgement within 72 hours.
- Within 14 days, a first assessment and a proposed disclosure date.
- A fix in a patch release, with credit in the release notes unless you prefer to stay anonymous.

Please give us a reasonable time to fix the problem before you disclose it.

## Hardening

Parsers are written in safe Rust (`#![forbid(unsafe_code)]`), check every size read from a file before allocating, and are fuzzed continuously. Known problems in dependencies and the fuzzing setup are described in [fuzz/README.md](fuzz/README.md).
