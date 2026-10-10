# Astapa Python SDK

Python SDK for the Astapa proxy - database change management with review workflows.

## Installation

```bash
pip install astapa
```

Or from source:

```bash
pip install -e /path/to/astapa/sdks/python
```

## Quick Start

```python
from astapa import Astapa

# Initialize client
client = Astapa(
    api_url="http://localhost:8080",
    proxy_host="localhost",
    proxy_port=5433,
)

# Create a session
session = client.create_session(project_id="my-project")

# Get a psycopg2 connection
conn = session.connect(database="mydb", user="user", password="pass")

# Make changes
cursor = conn.cursor()
cursor.execute("UPDATE users SET status = 'inactive' WHERE id = 1")
conn.commit()

# Review changes
diff = session.diff()
print(diff)

# Submit for review and approve
session.propose()
session.approve()  # or session.reject()
```

## Context Manager

For convenience, use the context manager which auto-connects and auto-proposes:

```python
with client.session(
    project_id="demo",
    database="mydb",
    user="user",
    password="pass",
) as session:
    session.execute("INSERT INTO logs (message) VALUES ('test')")
    # auto-proposes on successful exit

# Explicitly approve after reviewing
session.approve()
```

## API Reference

### Astapa

Main client class.

```python
client = Astapa(
    api_url="http://localhost:8080",  # Astapa API URL
    proxy_host="localhost",            # PostgreSQL proxy host
    proxy_port=5433,                   # PostgreSQL proxy port
    timeout=30.0,                      # HTTP request timeout
)
```

**Methods:**

- `create_session(project_id: str) -> Session` - Create a new session
- `get_session(session_id: str) -> Session` - Retrieve existing session
- `session(project_id, database, user, password, auto_propose=True)` - Context manager

### Session

Represents a Astapa session.

**Properties:**

- `id` - Session ID
- `schema_name` - Session's schema name
- `status` - Current status

**Methods:**

- `connect(database, user, password, **kwargs) -> connection` - Get psycopg2 connection
- `connection_string(database, user, password) -> str` - Get connection string
- `execute(query, params=None)` - Execute SQL on the connection
- `diff() -> dict` - View changes by table
- `mutations() -> list` - Chronological mutation log
- `propose() -> Session` - Submit for review
- `approve() -> Session` - Apply changes
- `reject() -> Session` - Discard changes
- `destroy()` - Delete the session
- `refresh() -> Session` - Refresh session data

## Connection String

If you prefer using your own database library, get a connection string:

```python
session = client.create_session(project_id="my-project")
conn_str = session.connection_string(database="mydb", user="user", password="pass")
# postgresql://user:pass@localhost:5433/mydb?options=-c%20astapa_session%3D<session-id>
```

## Requirements

- Python 3.9+
- requests
- psycopg2-binary
