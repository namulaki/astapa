# Astapa MCP Server Architecture

## Overview

The Astapa MCP Server provides a Model Context Protocol interface for a PostgreSQL change staging and review system. It enables AI assistants (like Claude Code) to safely interact with databases by **staging changes for human review** rather than applying them directly.

---

## System Architecture

```mermaid
flowchart TB
    subgraph Claude["Claude Code (AI Assistant)"]
        CC[Claude Code Client]
    end

    subgraph MCP["MCP Server Process"]
        Transport[StdioServerTransport<br/>stdin/stdout]
        Server[MCP Server<br/>@modelcontextprotocol/sdk]
        Handlers[Request Handlers]
        Session[Session Manager<br/>lazy initialization]
    end

    subgraph SDK["Astapa Node SDK"]
        Client[Astapa Client]
        SessionClass[Session Class]
    end

    subgraph External["External Services"]
        API[Astapa REST API<br/>port 8080]
        Proxy[PostgreSQL Proxy<br/>port 5433]
        PG[(PostgreSQL<br/>Database)]
    end

    CC <-->|JSON-RPC 2.0<br/>over stdio| Transport
    Transport <--> Server
    Server --> Handlers
    Handlers --> Session
    Session --> Client
    Client -->|HTTP| API
    SessionClass -->|PostgreSQL Wire Protocol| Proxy
    Proxy --> PG
    API -.->|Session Management| PG
```

---

## Tool Definitions

```mermaid
flowchart LR
    subgraph Tools["MCP Tools (4 total)"]
        Q[astapa_query<br/>SELECT only]
        M[astapa_mutate<br/>INSERT/UPDATE/DELETE]
        D[astapa_diff<br/>View pending changes]
        P[astapa_propose<br/>Submit for review]
    end

    subgraph Validation["SQL Validation"]
        VQ{Starts with<br/>SELECT?}
        VM{Starts with<br/>INSERT/UPDATE/DELETE?}
    end

    subgraph Actions["Session Actions"]
        Execute[session.execute]
        Diff[session.diff]
        Propose[session.propose]
    end

    Q --> VQ
    M --> VM
    VQ -->|Yes| Execute
    VQ -->|No| Error1[Error: Only SELECT allowed]
    VM -->|Yes| Execute
    VM -->|No| Error2[Error: Only mutations allowed]
    D --> Diff
    P --> Propose
```

---

## Request/Response Lifecycle

```mermaid
sequenceDiagram
    participant CC as Claude Code
    participant MCP as MCP Server
    participant SDK as Astapa SDK
    participant API as Astapa API
    participant Proxy as PG Proxy
    participant DB as PostgreSQL

    Note over CC,MCP: Initialization
    CC->>MCP: initialize (protocol version, capabilities)
    MCP-->>CC: capabilities (tools: {})
    CC->>MCP: notifications/initialized

    Note over CC,MCP: Tool Discovery
    CC->>MCP: tools/list
    MCP-->>CC: [astapa_query, astapa_mutate, astapa_diff, astapa_propose]

    Note over CC,DB: First Tool Call (Session Creation)
    CC->>MCP: tools/call {name: "astapa_query", sql: "SELECT..."}
    MCP->>SDK: createSession({projectId: "claude-code"})
    SDK->>API: POST /sessions
    API-->>SDK: {id: "session-123", ...}
    SDK-->>MCP: Session instance
    MCP->>SDK: session.connect(database, user, password)
    
    Note over CC,DB: Query Execution
    MCP->>SDK: session.execute("SELECT...")
    SDK->>Proxy: PostgreSQL query (with session header)
    Proxy->>DB: SELECT...
    DB-->>Proxy: Results
    Proxy-->>SDK: Results
    SDK-->>MCP: Results
    MCP-->>CC: {content: [{type: "text", text: JSON.stringify(results)}]}

    Note over CC,DB: Mutation (Staged)
    CC->>MCP: tools/call {name: "astapa_mutate", sql: "UPDATE..."}
    MCP->>SDK: session.execute("UPDATE...")
    SDK->>Proxy: PostgreSQL mutation (staged in sandbox)
    Proxy-->>SDK: OK (staged, not committed)
    MCP-->>CC: "Mutation staged for review"

    Note over CC,DB: Review Flow
    CC->>MCP: tools/call {name: "astapa_diff"}
    MCP->>SDK: session.diff()
    SDK->>API: GET /sessions/{id}/diff
    API-->>SDK: {tables: {...changes...}}
    MCP-->>CC: {content: [{type: "text", text: JSON.stringify(diff)}]}

    CC->>MCP: tools/call {name: "astapa_propose"}
    MCP->>SDK: session.propose()
    SDK->>API: POST /sessions/{id}/propose
    API-->>SDK: {status: "pending_review"}
    MCP-->>CC: "Session session-123 proposed for review"
```

---

## Data Flow

```mermaid
flowchart TD
    subgraph Input["Input Layer"]
        stdin[stdin]
    end

    subgraph Protocol["Protocol Layer"]
        JSONRPC[JSON-RPC 2.0 Parser]
        Router[Method Router]
    end

    subgraph Handlers["Handler Layer"]
        ListTools[ListToolsRequestSchema<br/>Returns tool definitions]
        CallTool[CallToolRequestSchema<br/>Executes tool calls]
    end

    subgraph Business["Business Logic"]
        SQLValidation[SQL Type Validation]
        SessionInit[Lazy Session Init]
        ToolSwitch[Tool Switch/Router]
    end

    subgraph External["External Calls"]
        HTTP[HTTP to Astapa API<br/>sessions, diff, propose]
        PGWire[PostgreSQL Wire Protocol<br/>via Proxy]
    end

    subgraph Output["Output Layer"]
        Response[JSON-RPC Response]
        stdout[stdout]
    end

    stdin --> JSONRPC
    JSONRPC --> Router
    Router -->|tools/list| ListTools
    Router -->|tools/call| CallTool
    ListTools --> Response
    CallTool --> SessionInit
    SessionInit --> SQLValidation
    SQLValidation --> ToolSwitch
    ToolSwitch -->|query/mutate| PGWire
    ToolSwitch -->|diff/propose| HTTP
    HTTP --> Response
    PGWire --> Response
    Response --> stdout
```

---

## Session State Machine

```mermaid
stateDiagram-v2
    [*] --> Uninitialized: Server Start
    
    Uninitialized --> Active: First Tool Call<br/>(createSession + connect)
    
    Active --> Active: query/mutate/diff
    
    Active --> PendingReview: propose()
    
    PendingReview --> Approved: approve()<br/>(human action)
    PendingReview --> Rejected: reject()<br/>(human action)
    
    Approved --> [*]: Changes Applied
    Rejected --> [*]: Changes Discarded
    
    Active --> [*]: destroy()
```

---

## Component Details

### MCP Server (`src/index.ts`)

| Component | Lines | Responsibility |
|-----------|-------|----------------|
| Environment Config | 11-16 | Load API URL, proxy host/port, DB credentials |
| Astapa Client | 18-22 | Initialize SDK client |
| MCP Server | 26-29 | Create server with tool capabilities |
| ListTools Handler | 31-72 | Return tool definitions with JSON schemas |
| CallTool Handler | 74-127 | Route and execute tool calls |
| Transport Setup | 129-133 | Connect stdio transport |

### Astapa SDK

```mermaid
classDiagram
    class Astapa {
        -apiUrl: string
        -proxyHost: string
        -proxyPort: number
        +createSession(options): Promise~Session~
        +getSession(id): Promise~Session~
    }

    class Session {
        -id: string
        -projectId: string
        -proxyHost: string
        -proxyPort: number
        +diff(): Promise~DiffResponse~
        +mutations(): Promise~MutationsResponse~
        +propose(): Promise~Session~
        +approve(): Promise~Session~
        +reject(): Promise~Session~
        +destroy(): Promise~void~
        +createPool(options): Pool
        +connectionConfig(options): ConnectionConfig
    }

    class DiffResponse {
        +tables: Record~string, TableDiff~
    }

    class TableDiff {
        +inserts: Row[]
        +updates: RowUpdate[]
        +deletes: Row[]
    }

    Astapa --> Session : creates
    Session --> DiffResponse : returns
    DiffResponse --> TableDiff : contains
```

---

## Configuration

```mermaid
flowchart LR
    subgraph Environment["Environment Variables"]
        E1[ASTAPA_API_URL<br/>default: localhost:8080]
        E2[ASTAPA_PROXY_HOST<br/>default: localhost]
        E3[ASTAPA_PROXY_PORT<br/>default: 5433]
        E4[DATABASE_NAME]
        E5[DATABASE_USER]
        E6[DATABASE_PASSWORD]
    end

    subgraph Config["Claude Code Config"]
        Settings[~/.claude/settings.json]
    end

    subgraph Server["MCP Server"]
        S[astapa-mcp]
    end

    Settings -->|spawns with env| S
    E1 --> S
    E2 --> S
    E3 --> S
    E4 --> S
    E5 --> S
    E6 --> S
```

### Example Configuration

```json
{
  "mcpServers": {
    "astapa": {
      "command": "astapa-mcp",
      "env": {
        "ASTAPA_API_URL": "http://localhost:8080",
        "ASTAPA_PROXY_HOST": "localhost",
        "ASTAPA_PROXY_PORT": "5433",
        "DATABASE_NAME": "myapp",
        "DATABASE_USER": "postgres",
        "DATABASE_PASSWORD": "secret"
      }
    }
  }
}
```

---

## Error Handling

```mermaid
flowchart TD
    subgraph Errors["Error Types"]
        E1[SQL Validation Error]
        E2[Unknown Tool Error]
        E3[SDK/API Error]
        E4[Connection Error]
    end

    subgraph Handling["Error Handling"]
        TryCatch[try-catch wrapper<br/>lines 77-126]
    end

    subgraph Response["Error Response Format"]
        R["{content: [{type: 'text', text: 'Error: ...'}]}"]
    end

    E1 --> TryCatch
    E2 --> TryCatch
    E3 --> TryCatch
    E4 --> TryCatch
    TryCatch --> R
```

---

## Key Design Decisions

1. **Staged Mutations**: All INSERT/UPDATE/DELETE operations are staged in a sandbox, not applied directly. This enables human review before changes affect production data.

2. **Lazy Session Initialization**: The session is created on first tool call, not at server startup. This avoids unnecessary connections when the server is loaded but not used.

3. **SQL Validation**: Simple prefix-based validation ensures queries and mutations are routed correctly. This is a safety guardrail, not a security boundary.

4. **Singleton Session**: One session per MCP server instance. The server is designed to be spawned per-conversation, so this simplifies state management.

5. **stdio Transport**: Using stdin/stdout allows Claude Code to spawn the server as a subprocess and communicate via JSON-RPC, following the standard MCP pattern.
