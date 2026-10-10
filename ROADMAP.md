# Astapa Roadmap

> Database isolation for AI agents

---

## Released

### v0.5.0 — Foundation

Core proxy functionality with copy-on-write isolation.

- **Wire Protocol** — Full PostgreSQL protocol support
- **Shadow Tables** — Copy-on-write isolation per session
- **Union Views** — Reads merge session changes with production
- **Conflict Detection** — Hash-based detection at approval time
- **Approval Workflow** — propose → approve/reject cycle
- **SQL Support** — JOINs, CTEs, subqueries, RETURNING, ON CONFLICT
- **Performance** — LRU cache, parallel infra creation, connection pooling
- **Observability** — Prometheus metrics, mutation logs, diff generation

---

### v0.6.0 — Proxy Modes

Two operational modes for different use cases.

| Mode | Behavior | Use Case |
|------|----------|----------|
| **WIPER** | Staged → discarded on disconnect | Dev, testing, CI/CD |
| **AGENTIC** | Staged → human approval → apply | Production agents |

- [x] Session mode enum (`wiper` / `agentic`)
- [x] Mode parameter in session creation API
- [x] Database schema with mode column
- [x] MCP server mode awareness
- [x] SDK support (Python, Node, Go)
- [ ] Parse mode from connection string
- [ ] Auto-destroy on disconnect (WIPER)

---

## In Progress

### v0.7.0 — Distribution

Get astapa into users' hands.

| Channel | Status |
|---------|--------|
| GitHub Releases | Cross-platform binaries via CI |
| Docker | `ghcr.io/namulaki/astapa` |
| npm | `@astapa/mcp-server` |
| PyPI | Soon `pip install astapa` |
| crates.io | Soon `cargo install astapa` |
| Homebrew | Soon `brew install astapa` |

- [x] Release workflow for cross-platform binaries
- [x] Docker image published to GHCR
- [x] MCP server on npm with OIDC publishing
- [ ] Python SDK on PyPI
- [ ] Rust crate on crates.io
- [ ] Homebrew tap

---

## Planned

### v0.8.0 — Webhooks & Integrations

Notify external systems on session events.

- [ ] Webhook delivery with HMAC signing
- [ ] Events: `session.created`, `session.proposed`, `session.approved`, `session.rejected`
- [ ] Retry with exponential backoff
- [ ] Slack integration (approve/reject from Slack)
- [ ] GitHub PR comments (show diff on PR)

---

### v0.9.0 — Enterprise

Features for teams and compliance.

- [ ] **Multi-tenant** — Project-based isolation
- [ ] **RBAC** — Role-based access (create, approve, admin)
- [ ] **Audit log** — Timestamped, attributed actions
- [ ] **TTL policies** — Auto-expire abandoned sessions
- [ ] **Partial approval** — Approve specific tables/rows
- [ ] **Session branching** — Fork from another session

---

### v1.0.0 — Production Ready

Battle-tested for real workloads.

- [ ] Large objects (BLOB/TOAST)
- [ ] Logical replication compatibility
- [ ] High availability (proxy clustering)
- [ ] Connection multiplexing
- [ ] Query timeout enforcement
- [ ] Rate limiting per project

---

## Future

### Ecosystem

- [ ] **CLI** — `astapa create`, `astapa approve`, `astapa diff`
- [ ] **VS Code extension** — Visual diff, approve from IDE
- [ ] **GitHub Action** — CI integration
- [ ] **ORM plugins** — Prisma, Drizzle, SQLAlchemy

### Cloud

- [ ] **Managed service** — astapa.com hosted offering
- [ ] **Supabase plugin** — Native integration
- [ ] **Neon integration** — Complement branching with staging

---

## Competitive Position

| | Astapa | Neon Branching | Manual Staging |
|---|--------|----------------|----------------|
| Setup | Seconds | Minutes | Hours |
| Storage | Delta only | Full copy | Full copy |
| Merge | Atomic | Manual | Manual |
| Conflicts | Automatic | None | None |
| Any Postgres | | Neon only | |

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Issues and PRs welcome.
