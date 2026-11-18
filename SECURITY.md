# Security Policy

## Supported versions

Oxreal is pre-release (Milestone 0). Security fixes go to the `main` branch only. Once releases
begin, this section will list which release lines receive fixes.

| Version | Supported |
|---|---|
| `main` | ✅ |
| Released versions | none yet |

## Reporting a vulnerability

**Do not open a public issue, discussion or pull request for security problems.**

Report privately through GitHub:
[**Report a vulnerability**](https://github.com/mrgyarmati/Oxreal-Engine/security/advisories/new)
(Security tab → "Report a vulnerability").

Please include:

- The affected component (crate, tool or file) and the version or commit.
- A description of the issue and its impact.
- Steps to reproduce or a proof of concept, such as a minimal asset file, project or packet capture.
- Your platform (Windows or Linux distribution) where relevant.

## What to expect

- We aim to acknowledge reports within **7 days**.
- We'll confirm the issue, agree on severity with you, and keep you informed of progress.
- Fixes are developed privately and released with a GitHub Security Advisory. With your
  permission we credit you. We'll request a CVE where appropriate.
- Please give us a reasonable window to release a fix before public disclosure. We aim for 90
  days or less and will coordinate the date with you.

## Scope

Security-relevant areas of Oxreal include, in particular:

- **Asset importers and file parsers** (glTF/GLB, images, audio, fonts, scene and project files).
  These handle untrusted input, so memory-safety issues, crashes, unbounded allocation or path
  traversal triggered by a crafted file are in scope.
- **Networking and replication** (once implemented): authority bypass, remote crashes,
  deserialization issues.
- **Plugin and gameplay-module loading**, and the editor automation/command interfaces.
- `unsafe` code, FFI and the Vulkan backend: soundness bugs reachable from safe APIs.
- The build, CI and release pipeline (supply chain).

Out of scope: vulnerabilities in third-party dependencies that are already publicly known and
tracked upstream (we still welcome a heads-up), and issues that require an already-compromised
machine.
