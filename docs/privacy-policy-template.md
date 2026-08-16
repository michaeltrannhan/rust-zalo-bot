# Privacy policy template (operator-facing)

Customize this template for your deployment. It is not legal advice.

## Data controller

**Operator:** [Your organization name]  
**Contact:** [privacy@example.com]

## What we store

- Account metadata (locale, timezone, retention preference)
- Expense records you create or confirm via the bot
- Receipt images for up to **7 days** (originals), then deleted per retention policy
- Operational logs via journald (no message bodies in operator CLI output)

## Third-party services

- **Zalo:** message delivery and webhook ingress
- **Google Gemini** (optional): receipt extraction when `extraction.backend = "gemini"`

## Your responsibilities

- Restrict bot access via `allowed_provider_sender_ids`
- Protect credential files and database backups
- Honor user deletion/export requests using built-in account jobs
- Publish this policy to end users if required in your jurisdiction

## Deletion and export

Users may request account deletion or export through bot commands supported by
this release. Deletion purges application data subject to backup retention.

## Backups

Database backups may retain data until backup rotation. Document your backup
schedule and restore testing in the operator runbook.

## Changes

Update this document when retention, providers, or hosting location changes.
