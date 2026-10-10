# Astapa Node.js SDK

Node.js/TypeScript SDK for [astapa](https://github.com/anthropics/astapa) — a PostgreSQL change staging and review system.

## Installation

```bash
npm install astapa pg
```

Requires Node.js 18+ (uses native fetch).

## Quick Start

```typescript
import { Astapa } from 'astapa';

// Initialize client
const client = new Astapa({
  apiUrl: 'http://localhost:8080',
  proxyHost: 'localhost',
  proxyPort: 5433,
});

// Create a session
const session = await client.createSession({ projectId: 'my-project' });

// Get a pg pool with session context
const pool = session.createPool({
  database: 'mydb',
  user: 'postgres',
  password: 'secret',
});

// Make changes (staged, not applied to main schema)
await pool.query('UPDATE users SET status = $1 WHERE id = $2', ['inactive', 1]);
await pool.query('INSERT INTO audit_log (action) VALUES ($1)', ['user_deactivated']);

// Review changes
const diff = await session.diff();
console.log('Changes by table:', diff.tables);

const mutations = await session.mutations();
console.log('Chronological log:', mutations.mutations);

// Submit for review and approve
await session.propose();
await session.approve(); // Changes applied to main schema

// Cleanup
await session.destroy();
```

## API Reference

### `Astapa`

Main client for interacting with astapa.

```typescript
const client = new Astapa({
  apiUrl: 'http://localhost:8080', // Required: astapa API URL
  proxyHost: 'localhost',          // Optional: PostgreSQL proxy host (default: 'localhost')
  proxyPort: 5433,                 // Optional: PostgreSQL proxy port (default: 5433)
});
```

#### Methods

- `createSession(options: { projectId: string }): Promise<Session>` — Create a new session
- `getSession(sessionId: string): Promise<Session>` — Retrieve an existing session

### `Session`

Represents an active astapa session.

#### Properties

- `id: string` — Session ID
- `schemaName: string` — Schema name used by this session
- `status: SessionStatus` — Current status ('active', 'proposed', 'approved', 'rejected', 'destroyed')
- `projectId: string` — Associated project ID

#### Methods

##### Database Connection

```typescript
// Get a pg Pool configured for this session
const pool = session.createPool({
  database: 'mydb',
  user: 'postgres',
  password: 'secret',
});

// Or get raw connection config
const config = session.connectionConfig({
  database: 'mydb',
  user: 'postgres',
  password: 'secret',
});
// config.options contains `-c astapa_session=<id>`
```

##### Review Changes

```typescript
// Get diff grouped by table
const diff = await session.diff();
// { session_id, tables: [{ table_name, inserts, updates, deletes, rows }] }

// Get chronological mutation log
const mutations = await session.mutations();
// { session_id, mutations: [{ id, timestamp, operation, table_name, row_data }] }
```

##### Lifecycle

```typescript
await session.propose();  // Submit for review
await session.approve();  // Apply changes to main schema
await session.reject();   // Discard changes
await session.destroy();  // Cleanup session (also ends all pools)
await session.refresh();  // Refresh session details from server
```

## Connection Options

The session ID is passed to PostgreSQL via the options parameter:

```
-c astapa_session=<session_id>
```

This allows the proxy to route queries to the correct staging schema.

## Error Handling

All methods throw errors with descriptive messages on failure:

```typescript
try {
  const session = await client.createSession({ projectId: 'test' });
} catch (error) {
  console.error('Failed to create session:', error.message);
}
```

## TypeScript Support

Full TypeScript types are included:

```typescript
import type {
  AstapaConfig,
  CreateSessionOptions,
  SessionDetails,
  SessionStatus,
  ConnectionConfig,
  DiffResponse,
  TableDiff,
  MutationsResponse,
  Mutation,
} from 'astapa';
```

## License

MIT
