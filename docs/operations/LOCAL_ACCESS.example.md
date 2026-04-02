---
last_updated: 2026-04-02
revision: 1
summary: Template for local machine-specific access notes that must not be committed
---

# Local Access Notes Template

## Purpose

This file is a local-only place for machine-specific or sensitive operational
notes. Copy it to `docs/operations/LOCAL_ACCESS.md` and keep the real file
untracked.

## Suggested Sections

## SSH And Host Aliases

- Preferred SSH aliases
- Usernames if they differ by host
- Notes about host key setup or `known_hosts`

## Service Endpoints

- Local-only URLs or ports
- Notes about which host is Forge, Remi, Crucible, or other infrastructure
- MCP endpoints that are safe to keep in a local note

## Credential Storage Locations

- Where credentials live locally
- How they are loaded
- Rotation reminders
- Any credentials that should be migrated out of legacy assistant config files

## Local Workflow Notes

- Workstation-specific caveats
- Useful local wrappers or shortcuts
- Deployment notes that are too sensitive or host-specific for tracked docs
