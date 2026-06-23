# Security Policy

## Reporting

Report security issues privately to the project maintainer. Do not open public issues for vulnerabilities, leaked credentials, subscription URLs, tokens, or customer data.

When reporting, include:

- Affected version or commit.
- Reproduction steps.
- Impact assessment.
- Any known mitigation.

## Secret Handling

- Do not commit subscription URLs, API tokens, SSH credentials, `.env` files, raw logs, or local acceptance reports.
- Use stdin for subscription import when possible.
- Keep local acceptance hosts, users, passwords, and subscription fixtures in a password manager, shell environment, or ignored local files.
- Redact `http://` and `https://` values from errors, logs, screenshots, and task notes unless the value is explicitly public.

## Supported Scope

Security fixes target the maintained `clash-tui` TUI/CLI and packaging flow. This branch intentionally does not provide a browser UI, public management API, WebSocket API, or static asset server.
