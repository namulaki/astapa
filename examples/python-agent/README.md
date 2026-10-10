# Python Agent Examples

Minimal examples showing how to integrate AI agents with Astapa.

## Setup

```bash
# Install dependencies
pip install astapa openai langchain langchain-openai

# Set environment variables
export DATABASE_NAME=<your_database>
export DATABASE_USER=<your_user>
export DATABASE_PASSWORD=<your_password>
export OPENAI_API_KEY=<your_api_key>

# Start the Astapa proxy (from repo root)
DATABASE_URL=postgres://<user>:<password>@localhost/<database> \
./target/release/astapa
```

## Examples

### Basic Session

```bash
python basic.py
```

Creates a session, executes some queries, and shows the staged changes.

### OpenAI Agent

```bash
python openai_agent.py "Refund order #102"
```

An agent that can query and modify the database. All writes are staged for approval.

### LangChain Agent

```bash
python langchain_agent.py "Find all admin users"
```

Same pattern using LangChain's tool abstraction.

### Multi-turn Chatbot

```bash
python chatbot.py
```

Interactive chatbot that maintains conversation history. Type `done` to propose changes or `quit` to discard.

## How it works

1. Agent connects through Astapa proxy
2. Reads see production data
3. Writes go to isolated shadow tables
4. Session proposes changes for review
5. Human approves → atomic merge to production

## Security Note

These examples include basic protections:
- SQL statement type validation (SELECT only for queries, INSERT/UPDATE/DELETE only for mutations)
- Dangerous keyword blocking (DROP, TRUNCATE, ALTER, CREATE, GRANT, REVOKE)
- Error handling for database and API operations

For production use, also consider parameterized queries and rate limiting.
