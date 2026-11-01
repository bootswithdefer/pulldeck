# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/bootswithdefer/pulldeck/compare/v0.1.0...v0.1.1) - 2026-11-01

### Added

- Add session state and a connection driver for the server
- Add settings and a token-store interface
- Add a client for the server's device-flow login
- Add the websocket transport and the client's copy of the wire format
- Add a live example for trying the transport against a server
- Keep the session token in the OS keyring
- Add a notification inbox model and an Inbox tab
- Connect server sessions to the Qt models, with a scripted demo server
- Start a server session from PULLDECK_SERVER and PULLDECK_TOKEN
- Add a sign-in dialog, saved sessions and sign-out
- Add request review, nudge and message actions to pull request rows

### Changed

- Expose pull requests as a list model that updates row by row
- Share one row-change path between refresh and server updates

### Fixed

- Give InboxView a typed formatter instead of a function property
