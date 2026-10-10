<div align="center">

```
   ___   _____  _____  ___  ____   ___  
  / _ \ / ____||_   _|/ _ \|  _ \ / _ \ 
 | |_| |\____ \  | | | |_| | |_) | |_| |
 |  _  | ____) | | | |  _  |  __/|  _  |
 |_| |_||_____/  |_| |_| |_|_|   |_| |_|
```

**Database isolation for AI agents**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![PostgreSQL](https://img.shields.io/badge/postgresql-14+-336791.svg)](https://www.postgresql.org)

[Documentation](https://astapa.com/docs) · [Getting Started](#quick-start) · [API Reference](#api-reference)

</div>

---

## The Problem

AI agents are writing to your database. Without guardrails, a single hallucination can corrupt production data, wipe tables, or create cascading failures. You need isolation—but spinning up database copies is slow, expensive, and doesn't reflect real data.

## The Solution

Astapa is a PostgreSQL proxy that creates **instant, copy-on-write sessions**. Agents read real data but write to isolated shadow tables. Changes are staged for review, then atomically merged—or discarded.

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Agent     │────▶│   Astapa    │────▶│  PostgreSQL │
│             │     │   Proxy     │     │             │
│ INSERT INTO │     │ ┌─────────┐ │     │ public.*    │
│ UPDATE ...  │     │ │ Session │ │     │ (untouched) │
│ DELETE ...  │     │ │ Shadow  │ │     │             │
└─────────────┘     │ │ Tables  │ │     └─────────────┘
                    │ └─────────┘ │
                    └─────────────┘
```

**Zero changes to your app. Zero changes to your schema. Just point at the proxy.**

---

## Two Modes

<table>
<tr>
<td width="50%">

### 🧪 WIPER Mode
**For development & testing**

All mutations staged in shadow tables, **automatically discarded** at session end.

- AI coding agents (Devin, Cursor, Claude Code)
- Integration tests
- CI/CD pipelines
- Local development

```bash
?mode=wiper
```

</td>
<td width="50%">

### 🛡️ AGENTIC Mode
**For production**

All mutations staged for **human review**. Nothing touches production until approved.

- AI agents on production data
- Bulk data operations
- Staging previews
- Compliance workflows

```bash
?mode=agentic
```

</td>
</tr>
</table>

---

## Quick Start

### Install

```bash
# Docker (recommended)
docker run -d \
  -e DATABASE_URL=postgres://user:pass@host:5432/db \
  -p 5433:5433 -p 8080:8080 \
  ghcr.io/namulaki/astapa

# Cargo
cargo install astapa

# From source
git clone https://github.com/namulaki/astapa && cd astapa
cargo build --release
```

### Connect

```bash
# Create a session
SESSION=$(curl -s -X POST http://localhost:8080/sessions \
  -H "Content-Type: application/json" \
  -d '{"project_id": "my-agent"}' | jq -r '.id')

# Connect through the proxy
psql "host=localhost port=5433 dbname=mydb options='-c astapa.session=$SESSION'"
```

### Use

```sql
-- All writes go to shadow tables
UPDATE users SET status = 'inactive' WHERE last_login < '2024-01-01';
DELETE FROM orders WHERE status = 'abandoned';

-- Reads see original data + your changes
SELECT * FROM users WHERE status = 'inactive';
```

### Review & Apply

```bash
# See what changed
curl http://localhost:8080/sessions/$SESSION/diff

# Apply to production (or reject)
curl -X POST http://localhost:8080/sessions/$SESSION/approve
```

---

## SDKs

<table>
<tr>
<td>

**Python**
```bash
pip install astapa
```
```python
from astapa import Astapa

client = Astapa(api_url="http://localhost:8080")
session = client.create_session(project_id="agent")

with session.connect(database="mydb") as conn:
    conn.execute("UPDATE users SET ...")

session.propose()  # Submit for review
```

</td>
<td>

**Node.js**
```bash
npm install astapa
```
```typescript
import { Astapa } from 'astapa';

const client = new Astapa({ apiUrl: 'http://localhost:8080' });
const session = await client.createSession({ projectId: 'agent' });

const pool = session.createPool({ database: 'mydb' });
await pool.query('UPDATE users SET ...');

await session.propose();  // Submit for review
```

</td>
</tr>
<tr>
<td>

**Go**
```bash
go get github.com/namulaki/astapa/sdks/go/astapa
```
```go
client := astapa.New("http://localhost:8080")
session, _ := client.CreateSession("agent")

db := session.Connect("mydb", "user", "pass")
db.Exec("UPDATE users SET ...")

session.Propose()  // Submit for review
```

</td>
<td>

**MCP Server** (Claude Code)
```bash
npm install -g @astapa/mcp-server
```
```json
{
  "mcpServers": {
    "astapa": {
      "command": "astapa-mcp",
      "env": {
        "ASTAPA_API_URL": "http://localhost:8080",
        "DATABASE_NAME": "mydb"
      }
    }
  }
}
```

</td>
</tr>
</table>

---

## How It Works

```mermaid
sequenceDiagram
    participant Agent
    participant Astapa
    participant PostgreSQL

    Agent->>Astapa: CREATE SESSION
    Astapa->>PostgreSQL: Create session_xxx schema
    Astapa-->>Agent: session_id

    Agent->>Astapa: UPDATE users SET status='inactive'
    Astapa->>PostgreSQL: Create shadow table (if first write)
    Astapa->>PostgreSQL: INSERT into session_xxx.users__shadow
    Astapa-->>Agent: 1 row affected

    Agent->>Astapa: SELECT * FROM users
    Astapa->>PostgreSQL: Query union view (original + shadow)
    Astapa-->>Agent: Results (with staged changes)

    Agent->>Astapa: APPROVE
    Astapa->>PostgreSQL: Check conflicts
    Astapa->>PostgreSQL: Atomic merge (UPSERT + DELETE)
    Astapa->>PostgreSQL: DROP session_xxx schema
    Astapa-->>Agent: Success
```

### Key Concepts

| Concept | Description |
|---------|-------------|
| **Session** | Isolated workspace with its own schema. All writes within a session go to shadow tables. |
| **Shadow Table** | Copy-on-write table that receives INSERTs/UPDATEs/DELETEs. Created lazily on first write. |
| **Union View** | Combines original table rows (excluding modified PKs) with shadow table rows. Reads go here. |
| **Approval** | Atomic merge of shadow changes into source tables. Includes conflict detection and FK validation. |

---

## API Reference

### Sessions

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/sessions` | Create session. Body: `{"project_id": "...", "mode": "agentic"}` |
| `GET` | `/sessions/:id` | Get session details |
| `DELETE` | `/sessions/:id` | Destroy session immediately |

### Workflow

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/sessions/:id/propose` | Submit for review (blocks further writes) |
| `POST` | `/sessions/:id/approve` | Apply changes to source tables |
| `POST` | `/sessions/:id/reject` | Discard all changes |

### Inspection

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/sessions/:id/diff` | All changes grouped by table |
| `GET` | `/sessions/:id/mutations` | Chronological mutation log |

### Webhooks

Configure `WEBHOOK_URL` to receive events:

```json
{
  "event": "session.proposed",
  "session_id": "abc123",
  "project_id": "my-agent",
  "diff": { "tables": [...] }
}
```

Events: `session.created`, `session.proposed`, `session.approved`, `session.rejected`, `session.destroyed`

---

## Configuration

```bash
# Required
DATABASE_URL=postgres://user:pass@localhost:5432/mydb

# Server (defaults shown)
PROXY_ADDR=0.0.0.0:5433          # PostgreSQL wire protocol
API_ADDR=0.0.0.0:8080            # HTTP API
SESSION_TTL_SECONDS=3600         # Auto-cleanup after 1 hour

# Webhooks
WEBHOOK_URL=https://your-app.com/hooks
WEBHOOK_SECRET=your-hmac-secret  # Signs payloads with HMAC-SHA256

# Mode default
ASTAPA_DEFAULT_MODE=agentic      # 'wiper' or 'agentic'
```

---

## Observability

Prometheus metrics at `GET /metrics`:

| Metric | Type | Description |
|--------|------|-------------|
| `astapa_proxy_connections_active` | Gauge | Current proxy connections |
| `astapa_queries_total` | Counter | Queries by type (select/insert/update/delete) |
| `astapa_query_duration_seconds` | Histogram | Query latency distribution |
| `astapa_sessions_total` | Counter | Sessions by status |
| `astapa_approvals_total` | Counter | Approvals by result (success/conflict/error) |

---

## Limitations

- **No DDL** — CREATE, ALTER, DROP blocked within sessions
- **Sequences** — Serial columns may have gaps after approval
- **Large sessions** — Millions of rows may be slow to approve
- **No replication** — Logical replication not supported

---

## Development

```bash
# Prerequisites
docker compose up -d postgres   # or your own PostgreSQL 14+

# Build & test
cargo build
cargo test

# Run locally
RUST_LOG=debug cargo run

# Lint
cargo fmt --check
cargo clippy
```

---

## License

[MIT](LICENSE)

---

<div align="center">

**[Documentation](https://astapa.com/docs)** · **[GitHub](https://github.com/namulaki/astapa)** · **[Discord](https://discord.gg/astapa)**

</div>
