# Astapa Roadmap

## Current State (v0.1 - Proof of Concept)
- [x] Wire protocol proxy (TCP, message parsing)
- [x] Session creation via HTTP API
- [x] Schema-based isolation (`SET search_path`)
- [x] Basic query interception
- [x] Frontend playground demo

**What works:** Simple queries isolated per-session.  
**What breaks:** Reads don't merge with production. No conflict detection. Complex SQL.

---

## Phase 1: Copy-on-Write Foundation (v0.2) ✅
*Make isolation actually work*

- [x] **Shadow tables** — Auto-create `_shadow_{table}` on first write (PR #5)
- [x] **Deleted tracking** — `_deleted_{table}` for DELETE operations (PR #1)
- [x] **Union views** — `CREATE VIEW {table} AS shadow UNION ALL (prod EXCEPT deleted)` (PR #2)
- [x] **Write interception** — INSERT/UPDATE/DELETE → redirect to shadow tables
- [x] **Read-through** — SELECT sees merged session + production data
- [x] **Hash tracking** — Store row hashes at fork time for conflict detection (PR #5)

**Exit criteria:** Can INSERT in session, SELECT sees it merged with prod, prod unchanged.

**Status:** Phase 1 complete. Rewriter integrated into connection handler with dynamic infrastructure creation. Queries are analyzed, tables extracted, and views/shadow tables created on-demand before execution.

---

## Phase 2: Conflict Detection & Approval (v0.3) ✅
*Safe merge back to production*

- [x] **Conflict detection** — Compare fork-time hash vs current production hash (PR #8)
- [x] **Conflict types** — Modified, deleted, constraint violation (PR #8)
- [x] **Approval API** — `POST /sessions/:id/approve` with atomic apply (PR #8)
- [x] **Reject API** — `POST /sessions/:id/reject` drops schema cleanly (PR #7)
- [x] **Mutation log** — Track all changes for review UI (PR #9)
- [x] **Diff generation** — Human-readable before/after for approval UI (PR #10)

**Exit criteria:** Full create → modify → approve/reject cycle works.

**Status:** Phase 2 complete. Approval applies shadow changes to production atomically with conflict detection. Reject drops session schema cleanly. Mutation log and diff generation provide visibility into staged changes.

---

## Phase 3: SQL Completeness (v0.4) ✅
*Handle real-world queries*

- [x] **JOIN rewriting** — Multi-table queries across session/prod boundaries (PR #11)
- [x] **Subqueries & CTEs** — Recursive, lateral, window functions (PR #11)
- [x] **RETURNING clause** — Capture returned data from writes (PR #11)
- [x] **ON CONFLICT** — Upsert semantics in shadow tables (PR #11)
- [x] **Prepared statements** — Extended query protocol fully supported (PR #11)
- [x] **Transactions** — BEGIN/COMMIT/ROLLBACK within session (PR #11)
- [x] **pg_query integration** — Deferred; sqlparser 0.41 handles all tested Postgres syntax

**Exit criteria:** pgbench, Prisma, Drizzle queries all work.

**Status:** Phase 3 complete. pgbench verified working through proxy (5/5 transactions, 0 failures). Playground frontend uses proxy for session isolation. sqlparser handles arrays, JSON operators, type casts, LATERAL, window functions, DISTINCT ON, FILTER, recursive CTEs, FOR UPDATE, and INTERVAL.

---

## Phase 4: Production Hardening (v0.5) ✅
*Ready for real workloads*

- [x] **Connection pooling** — PgBouncer compatibility via SET astapa.session (PR #15)
- [x] **Performance** — LRU query cache, parallel infra creation, 16KB buffers (PR #19)
- [x] **Sequence handling** — Prevent ID collisions on approval (PR #14)
- [x] **Foreign keys** — Validate constraints at approval time (PR #15)
- [ ] **Large objects** — BLOB/TOAST support
- [x] **COPY protocol** — Bulk import/export (PR #16)
- [x] **Graceful shutdown** — Drain connections, signal handling (PR #17)
- [x] **Observability** — Prometheus metrics, cache hit/miss tracking (PR #18)
- [x] **Session expiration** — PendingReview sessions now expire correctly (PR #21)

**Exit criteria:** Can run production workload for 24h without issues.

**Status:** Phase 4 complete. All core hardening done. Only large objects (BLOB/TOAST) deferred to future release.

---

## Phase 4.5: Open Source Release (v0.5.1) ✅
*Ship it*

- [x] **LICENSE** — MIT license (PR #20)
- [x] **CI** — GitHub Actions for fmt, clippy, test (PR #20)
- [x] **CONTRIBUTING.md** — Contribution guidelines (PR #20)
- [x] **README** — Professional documentation with ASCII logo, API reference, examples (PR #22)
- [x] **Pre-push hooks** — Local CI checks before push
- [x] **GitHub Release** — Tag v0.5.0, release notes
- [ ] **Docker image** — Published to ghcr.io
- [ ] **crates.io** — `cargo install astapa`
- [ ] **GitHub Release binaries** — Cross-compiled Linux/macOS/Windows via CI
- [ ] **Homebrew tap** — `brew install astapa`

**Status:** v0.5.0 released. Repo is public.

---

## Proxy Modes (v0.6)
*Two modes for different use cases*

### Overview

Astapa supports two operational modes to cover different use cases:

| Mode | Behavior | Use Case |
|------|----------|----------|
| **WIPER** | Staged → discarded | Dev/testing, disposable sandbox |
| **AGENTIC** | Staged → human approval → apply | Production agents, data ops |

Both modes use shadow tables for complete isolation. Astapa strictly owns safety and staging — there is no passthrough mode.

### WIPER_MODE (Default for development)

All mutations are staged in shadow tables but **never applied to production**. Session is ephemeral and discarded at end. Ideal for:
- AI-assisted feature development
- Integration tests against real data
- CI/CD test runs
- Experimentation without consequences

```
Agent writes code → Tests against real data → Session destroyed
```

### AGENTIC_MODE (Default for production)

All mutations are staged and **require human approval** via `propose()` → `approve()` workflow. Current behavior. Ideal for:
- AI agents handling customer data
- Data operations (refunds, corrections)
- Bulk updates requiring review

```
Agent proposes changes → Human reviews diff → Approve or reject
```

### Implementation Plan

**1. Connection-level mode selection**
```
postgresql://user:pass@proxy:5433/db?mode=wiper
postgresql://user:pass@proxy:5433/db?mode=agentic
```

**2. Environment variable defaults**
- `ASTAPA_DEFAULT_MODE=wiper` for local/dev
- `ASTAPA_DEFAULT_MODE=agentic` for production

**3. MCP server mode awareness**
- Mode passed in tool descriptions
- Agent sees: "Current Mode: AGENTIC_MODE. Mutations require approval."
- Prevents confusion about why writes aren't immediately visible

**4. Visual distinction**
- WIPER: Purple/gray (ephemeral, safe)
- AGENTIC: Yellow/amber (pending review)

### Tasks

- [x] Add `mode` parameter to session creation API
- [x] Add `SessionMode` enum to session manager
- [x] Update database schema with mode column
- [x] Add mode to MCP server config and tool responses
- [ ] Parse mode from connection string options
- [ ] Implement WIPER_MODE (auto-destroy on disconnect)
- [ ] Update docs and site messaging

---

## Phase 5: Enterprise Features (v0.6)
*Sell to companies*

- [ ] **Multi-tenant** — Multiple projects, isolated session pools
- [ ] **RBAC** — Who can create/approve/reject sessions
- [ ] **Audit log** — Every action timestamped and attributed
- [ ] **Webhooks** — Notify external systems on session events
- [ ] **TTL policies** — Auto-expire abandoned sessions
- [ ] **Session branching** — Fork a session from another session
- [ ] **Partial approval** — Approve some mutations, reject others

**Exit criteria:** Enterprise pilot customer in production.

---

## Phase 6: Ecosystem & Cloud (v1.0)
*Moat through integration*

- [ ] **Hosted offering** — astapa.io managed service
- [ ] **Supabase integration** — Plugin/extension
- [ ] **Neon integration** — Branching comparison/complement
- [ ] **ORM adapters** — Prisma, Drizzle, SQLAlchemy plugins
- [ ] **CLI tool** — `astapa create`, `astapa approve`, `astapa diff`
- [ ] **VS Code extension** — Visual diff, approve from IDE
- [ ] **GitHub Action** — CI/CD integration for schema changes

**Exit criteria:** 100+ production users, self-sustaining growth.

---

## Timeline Estimate

| Phase | Effort | Cumulative |
|-------|--------|------------|
| v0.2 Copy-on-Write | 2-3 weeks | 3 weeks |
| v0.3 Conflict Detection | 2 weeks | 5 weeks |
| v0.4 SQL Completeness | 4-6 weeks | 11 weeks |
| v0.5 Production Hardening | 4 weeks | 15 weeks |
| v0.6 Enterprise | 6 weeks | 21 weeks |
| v1.0 Ecosystem | Ongoing | — |

**Critical path:** v0.4 (SQL completeness) is the hardest. pg_query integration + JOIN rewriting is where most projects die.

---

## Competitive Positioning

| Approach | Astapa | Neon Branching | Manual Staging DB |
|----------|---------|----------------|-------------------|
| Setup time | Seconds | Minutes | Hours/Days |
| Storage cost | Delta only | Full copy | Full copy |
| Merge to prod | Atomic | Manual migration | Manual migration |
| Conflict detection | Automatic | None | None |
| Works with any Postgres | Yes | Neon only | Yes |

**Pitch:** "Database sandbox for AI agents. Fork, experiment, approve changes — without touching production."
