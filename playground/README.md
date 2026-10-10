# Astapa Playground

AI-powered database assistant with sandboxed operations. All changes are isolated until you approve them.

**Powered by [LangChain](https://langchain.com) SQL Agent**

## Setup

```bash
cd playground
python -m venv venv
source venv/bin/activate  # or `venv\Scripts\activate` on Windows
pip install -r requirements.txt
cp .env.example .env
# Edit .env with your OpenAI API key
```

## Usage

Make sure the astapa proxy is running:
```bash
cd .. && cargo run --release
```

Then run the agent:
```bash
python agent.py
```

### Interactive Commands

- Type natural language queries: "Show me all users with balance > 100"
- Make changes: "Refund $50 to Alice's account"
- `diff` - See staged changes
- `approve` - Apply changes to production
- `reject` - Discard changes
- `quit` - Exit (discards session)

### Programmatic Usage

```python
from agent import AstapaSession, create_astapa_agent

session = AstapaSession(project_id="my-project")
session.create()

agent = create_astapa_agent(session)
result = agent.invoke({"input": "Show me the top 5 customers by balance"})
print(result["output"])

# Review and approve
session.approve()
```

## How It Works

1. Agent connects through astapa proxy with a session ID
2. All writes go to shadow tables (production unchanged)
3. You review the diff
4. Approve → changes applied atomically with conflict detection
5. Reject → shadow tables dropped, nothing changed

## Conflict Detection

If another user modified the same rows since your session started:
```
Conflicts detected:
  - users pk=1: row_modified
```

Resolve by rejecting the session and retrying with fresh data.
