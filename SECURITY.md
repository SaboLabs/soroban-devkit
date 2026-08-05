# Security Policy

This document describes security practices and how to report vulnerabilities.

## Supported Versions

Only the latest minor version is actively maintained. Security fixes are backported sparingly, prioritizing the latest stable release.

| Version | Supported          |
| :------ | :----------------- |
| 1.0.x   | :white_check_mark: |
| < 1.0   | :x:                |

## Reporting a Vulnerability

Report suspected security vulnerabilities by opening a **Private Security Advisory** on the project's GitHub repository (preferred): https://github.com/naninu123/soroban-devkit/security/advisories/new. If you cannot use advisories, email the maintainer at security@naninu123.dev (set up a real forwarding address before relying on this).

Include:
- A concise description of the issue
- The component and version affected
- A minimal reproduction (code sample, command, input payload)
- Impact assessment (who/what is affected)
- Any mitigations you've identified

Do not include exploit code in public issues. Publicly disclosing active exploits before mitigation is released harms users.

## Data Handling Notes

This tool operates primarily offline for decoding purposes — it does not transmit payloads unless explicitly configured to do so (e.g., a future `sdkt inspect` feature using RPC URLs). Payloads provided on the command line or in files are decoded and printed, not sent over the network by default.

Secret management:
- Do not commit private keys, mnemonics, or sensitive passphrases
- Network configuration includes RPC URL / passphrase; use local development overrides or environment-aware configuration for production
