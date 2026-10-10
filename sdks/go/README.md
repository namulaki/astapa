# Astapa Go SDK

Go client library for [astapa](https://github.com/apridosimarmata/astapa) — a PostgreSQL proxy that enables safe, reviewable database changes through shadow schemas.

## Installation

```bash
go get github.com/apridosimarmata/astapa/sdks/go/astapa
```

## Quick Start

```go
package main

import (
    "context"
    "log"

    "github.com/apridosimarmata/astapa/sdks/go/astapa"
)

func main() {
    ctx := context.Background()

    // Initialize client
    client := astapa.NewClient(astapa.Config{
        APIURL:    "http://localhost:8080",
        ProxyHost: "localhost",
        ProxyPort: 5433,
    })

    // Create a session
    session, err := client.CreateSession(ctx, "my-project")
    if err != nil {
        log.Fatal(err)
    }
    defer session.Destroy(ctx)

    // Get a database connection with session context
    db, err := session.Connect(ctx, "mydb", "user", "password")
    if err != nil {
        log.Fatal(err)
    }
    defer db.Close()

    // Make changes — they go to the shadow schema
    _, err = db.ExecContext(ctx, "UPDATE users SET status = $1 WHERE id = $2", "inactive", 1)
    if err != nil {
        log.Fatal(err)
    }

    // Review changes
    diff, err := session.Diff(ctx)
    if err != nil {
        log.Fatal(err)
    }
    for _, table := range diff.Tables {
        log.Printf("Table %s: %d inserts, %d updates, %d deletes",
            table.Table, table.Inserts, table.Updates, table.Deletes)
    }

    // Submit for review and approve
    if err := session.Propose(ctx); err != nil {
        log.Fatal(err)
    }
    if err := session.Approve(ctx); err != nil {
        log.Fatal(err)
    }
}
```

## API Reference

### Client

```go
// Create a new client
client := astapa.NewClient(astapa.Config{
    APIURL:     "http://localhost:8080",  // astapa API
    ProxyHost:  "localhost",               // PostgreSQL proxy host
    ProxyPort:  5433,                      // PostgreSQL proxy port
    HTTPClient: nil,                       // Optional custom http.Client
})

// Create a new session
session, err := client.CreateSession(ctx, "project-id")

// Get an existing session
session, err := client.GetSession(ctx, "session-id")
```

### Session

```go
// Get connection string (DSN format)
connStr := session.ConnectionString("dbname", "user", "password")
// => "host=localhost port=5433 dbname=... options='-c astapa_session=...'"

// Get connection string (URL format)
connURL := session.ConnectionStringURL("dbname", "user", "password")
// => "postgres://user:password@localhost:5433/dbname?options=..."

// Open a database connection
db, err := session.Connect(ctx, "dbname", "user", "password")

// View changes grouped by table
diff, err := session.Diff(ctx)

// View chronological mutation log
mutations, err := session.Mutations(ctx)

// Refresh session data from API
err := session.Refresh(ctx)

// Workflow actions
err := session.Propose(ctx)  // Submit for review
err := session.Approve(ctx)  // Apply changes to main DB
err := session.Reject(ctx)   // Discard changes

// Cleanup
err := session.Destroy(ctx)
```

### Types

```go
// Session status constants
astapa.StatusActive   // Session is active, accepting changes
astapa.StatusProposed // Session submitted for review
astapa.StatusApproved // Changes applied to main database
astapa.StatusRejected // Changes discarded

// DiffResponse contains changes grouped by table
type DiffResponse struct {
    SessionID string
    Tables    []TableDiff
}

type TableDiff struct {
    Table   string
    Inserts int
    Updates int
    Deletes int
    Rows    []Row
}

// MutationsResponse contains chronological change log
type MutationsResponse struct {
    SessionID string
    Mutations []Mutation
}
```

### Error Handling

API errors are returned as `*astapa.APIError`:

```go
session, err := client.CreateSession(ctx, "project-id")
if err != nil {
    if apiErr, ok := err.(*astapa.APIError); ok {
        log.Printf("API error %d: %s", apiErr.StatusCode, apiErr.Message)
    }
    return err
}
```

## Using with Other PostgreSQL Drivers

The SDK uses `lib/pq` by default, but you can use any PostgreSQL driver with the connection string:

```go
// With pgx
import "github.com/jackc/pgx/v5"

connStr := session.ConnectionString("mydb", "user", "pass")
conn, err := pgx.Connect(ctx, connStr)

// With pgxpool
import "github.com/jackc/pgx/v5/pgxpool"

pool, err := pgxpool.New(ctx, session.ConnectionStringURL("mydb", "user", "pass"))
```

## License

MIT
