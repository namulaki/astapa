<div align="center">

```
                        _       _           
  _ __ ___   __ _ _ __ | | ___ | |__  _   _ 
 | '_ ` _ \ / _` | '__|| |/ _ \| '_ \| | | |
 | | | | | | (_| | |   | | (_) | |_) | |_| |
 |_| |_| |_|\__,_|_|   |_|\___/|_.__/ \__,_|
                                      proxy
```

**Copy-on-write session isolation for PostgreSQL**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

</div>

## Overview

astapa sits between your application and PostgreSQL, intercepting queries and routing them through isolated sessions. Within a session:

- **Reads** see the original data plus any uncommitted changes
- **Writes** go to shadow tables, leaving production data untouched
- **Approval** atomically merges session changes into the source tables

This enables workflows like data review, staging environments, and safe bulk operations—without schema changes or application modifications.

## Use cases

| Scenario | How astapa helps |
|----------|-------------------|
| **Data review** | Analysts modify data in isolation; reviewers approve before production |
| **Safe bulk operations** | Run large UPDATE/DELETE, verify results, then apply |
| **Testing with production data** | Point tests at the proxy for realistic data without risk |
| **AI agent sandboxing** | Let LLM agents query and modify data with human-in-the-loop approval |

## Quick start

### Prerequisites

- Rust 1.75+ (for building)
- PostgreSQL 14+

### Installation

**Proxy**

```bash
# Via crates.io
cargo install astapa

# Via Docker
docker pull ghcr.io/aprdos/astapa

# From source
git clone https://github.com/namulaki/astapa.git
cd astapa
cargo build --release
```

**SDKs**

```bash
# Node.js
npm install astapa

# Python
pip install astapa

# Go
go get github.com/namulaki/astapa/sdks/go/astapa
```

**MCP Server** (for Claude Code integration)

```bash
npm install -g @astapa/mcp-server
```

### Configuration

```bash
# Required
export DATABASE_URL=postgres://user:pass@localhost:5432/mydb

# Optional (showing defaults)
export PROXY_ADDR=0.0.0.0:5433        # PostgreSQL wire protocol
export API_ADDR=0.0.0.0:8080          # HTTP API
export SESSION_TTL_SECONDS=3600       # 1 hour

# Webhooks (optional)
export WEBHOOK_URL=https://your-app.com/hooks/astapa
export WEBHOOK_SECRET=your-hmac-secret
```

### Run

```bash
./target/release/astapa
```

### Basic workflow

```bash
# 1. Create a session
SESSION_ID=$(curl -s -X POST http://localhost:8080/sessions \
  -H "Content-Type: application/json" \
  -d '{"project_id": "my-project"}' | jq -r '.id')

# 2. Connect through the proxy with session context
psql "host=localhost port=5433 dbname=mydb options='-c astapa_session=$SESSION_ID'"

# 3. Make changes (isolated to this session)
UPDATE users SET status = 'inactive' WHERE last_login < '2024-01-01';

# 4. Review what changed
curl http://localhost:8080/sessions/$SESSION_ID/diff

# 5. Apply to production
curl -X POST http://localhost:8080/sessions/$SESSION_ID/approve
```

## How it works

```mermaid
flowchart LR
    subgraph Clients
        A[psql / app / ORM]
    end
    
    subgraph astapa
        B[SQL Parser] --> C[Table Rewriter]
        C --> D[Session Manager]
    end
    
    subgraph PostgreSQL
        E[public.* tables]
        F[session_* schemas]
    end
    
    A --> B
    D --> E
    D --> F
```

When a session first writes to a table:

1. **Shadow table** created with identical schema in `session_{id}` schema
2. **Union view** created: source rows (minus modified PKs) + shadow rows
3. **Reads** go through the view; **writes** go to the shadow table

On approval:

1. Conflict detection (concurrent modifications to same rows)
2. Foreign key validation
3. Atomic merge: deletes applied, shadow rows upserted
4. Session schema dropped

## API reference

### Sessions

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/sessions` | Create session. Body: `{"project_id": "..."}` |
| `GET` | `/sessions/:id` | Get session details and status |
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

### Operations

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/health` | Health check |
| `GET` | `/metrics` | Prometheus metrics |

## Client connection

Pass the session ID via connection options:

```python
# Python (psycopg2)
conn = psycopg2.connect(
    host="localhost", port=5433,
    database="mydb", user="user", password="pass",
    options=f"-c astapa_session={session_id}"
)
```

```javascript
// Node.js (pg)
const client = new Client({
  host: 'localhost', port: 5433,
  database: 'mydb', user: 'user', password: 'pass',
  options: `-c astapa_session=${sessionId}`
});
```

```bash
# psql
psql "host=localhost port=5433 dbname=mydb options='-c astapa_session=...'"
```

## Observability

Prometheus metrics at `/metrics`:

| Metric | Type | Description |
|--------|------|-------------|
| `astapa_proxy_connections_active` | Gauge | Current connections |
| `astapa_queries_total` | Counter | Queries by type and status |
| `astapa_query_duration_seconds` | Histogram | Query latency |
| `astapa_sessions_total` | Counter | Sessions by status |
| `astapa_approvals_total` | Counter | Approvals by result |

## Limitations

- **No DDL**: CREATE, ALTER, DROP blocked within sessions
- **Sequences**: Serial columns may have gaps after approval
- **Scale**: Very large sessions (millions of rows) may be slow to approve
- **Replication**: No support for logical replication or streaming

## Development

```bash
# Prerequisites
make postgres          # Start local PostgreSQL

# Build and test
cargo build
cargo test

# Lint
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings

# Run locally
RUST_LOG=debug cargo run
```

## Contributing

Contributions welcome. Please open an issue first to discuss significant changes.

## License

[MIT](LICENSE)
