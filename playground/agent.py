"""
Astapa SQL Agent - LangChain integration for sandboxed database operations.

All queries run through the astapa proxy, isolating changes to a session.
Changes can be reviewed and approved/rejected before hitting production.
"""

import os
import requests
from typing import Optional

from dotenv import load_dotenv
from langchain_community.utilities import SQLDatabase
from langchain_community.agent_toolkits import create_sql_agent
from langchain_openai import ChatOpenAI

load_dotenv()

ASTAPA_API = os.getenv("ASTAPA_API_URL", "http://localhost:8080")
ASTAPA_PROXY = os.getenv("ASTAPA_PROXY_HOST", "localhost")
ASTAPA_PROXY_PORT = os.getenv("ASTAPA_PROXY_PORT", "5433")
DATABASE_NAME = os.getenv("DATABASE_NAME", "astapa_playground")
DATABASE_USER = os.getenv("DATABASE_USER", "postgres")
DATABASE_PASSWORD = os.getenv("DATABASE_PASSWORD", "")

# LLM config - supports OpenAI-compatible endpoints
LLM_API_KEY = os.getenv("OPENAI_API_KEY") or os.getenv("ANTHROPIC_API_KEY")
LLM_BASE_URL = os.getenv("OPENAI_BASE_URL") or os.getenv("ANTHROPIC_BASE_URL")
if LLM_BASE_URL and not LLM_BASE_URL.endswith("/v1"):
    LLM_BASE_URL = f"{LLM_BASE_URL}/v1"
LLM_MODEL = os.getenv("LLM_MODEL", "claude-sonnet-4-20250514")


class AstapaSession:
    """Manages a astapa session with create/approve/reject lifecycle."""

    def __init__(self, project_id: str = "langchain-agent"):
        self.project_id = project_id
        self.session_id: Optional[str] = None
        self.schema_name: Optional[str] = None

    def create(self) -> str:
        """Create a new session and return session_id."""
        resp = requests.post(
            f"{ASTAPA_API}/sessions",
            json={"project_id": self.project_id}
        )
        resp.raise_for_status()
        data = resp.json()
        self.session_id = data["session_id"]
        self.schema_name = data["schema_name"]
        print(f"Created session: {self.session_id}")
        return self.session_id

    def get_status(self) -> dict:
        """Get current session status."""
        resp = requests.get(f"{ASTAPA_API}/sessions/{self.session_id}")
        resp.raise_for_status()
        return resp.json()

    def get_diff(self) -> dict:
        """Get staged changes diff."""
        resp = requests.get(f"{ASTAPA_API}/sessions/{self.session_id}/diff")
        resp.raise_for_status()
        return resp.json()

    def propose(self) -> dict:
        """Submit session for review."""
        resp = requests.post(f"{ASTAPA_API}/sessions/{self.session_id}/propose")
        resp.raise_for_status()
        return resp.json()

    def approve(self) -> dict:
        """Approve and apply changes to production."""
        self.propose()
        resp = requests.post(f"{ASTAPA_API}/sessions/{self.session_id}/approve")
        resp.raise_for_status()
        return resp.json()

    def reject(self) -> dict:
        """Reject and discard changes."""
        resp = requests.post(f"{ASTAPA_API}/sessions/{self.session_id}/reject")
        resp.raise_for_status()
        return resp.json()

    def get_connection_string(self) -> str:
        """Get SQLAlchemy connection string through proxy."""
        return (
            f"postgresql://{DATABASE_USER}:{DATABASE_PASSWORD}"
            f"@{ASTAPA_PROXY}:{ASTAPA_PROXY_PORT}/{DATABASE_NAME}"
        )

    def get_connect_args(self) -> dict:
        """Get connection args with session ID."""
        return {"options": f"-c astapa_session={self.session_id}"}


def create_astapa_agent(
    session: AstapaSession,
    model: str = None,
    temperature: float = 0,
    verbose: bool = True
):
    """Create a LangChain SQL agent connected through astapa proxy."""

    if not session.session_id:
        session.create()

    db = SQLDatabase.from_uri(
        session.get_connection_string(),
        engine_args={"connect_args": session.get_connect_args()}
    )

    llm = ChatOpenAI(
        model=model or LLM_MODEL,
        temperature=temperature,
        api_key=LLM_API_KEY,
        base_url=LLM_BASE_URL
    )

    agent = create_sql_agent(
        llm,
        db=db,
        agent_type="openai-tools",
        verbose=verbose,
        prefix="""You are an AI assistant with access to a sandboxed database.
All your changes are isolated - they won't affect production until approved.
Feel free to experiment with queries and modifications.

When asked to make changes, do them confidently. The human will review before applying.
Always explain what you did after completing a task."""
    )

    return agent


def interactive_session():
    """Run an interactive session with the agent."""
    print("=" * 60)
    print("Astapa Playground - Sandboxed Database Assistant")
    print("Powered by LangChain SQL Agent")
    print("=" * 60)
    print()

    session = AstapaSession()
    session.create()

    agent = create_astapa_agent(session, verbose=False)

    print(f"Session ID: {session.session_id}")
    print("All changes are sandboxed. Type 'approve' to apply, 'reject' to discard.")
    print("Type 'diff' to see staged changes, 'quit' to exit.")
    print()

    while True:
        try:
            user_input = input("You: ").strip()
        except (EOFError, KeyboardInterrupt):
            print("\nGoodbye!")
            break

        if not user_input:
            continue

        if user_input.lower() == "quit":
            print("Discarding session...")
            session.reject()
            break

        if user_input.lower() == "approve":
            result = session.approve()
            if result.get("status") == "approved":
                print(f"Changes applied to production! ({result.get('applied', 0)} operations)")
            elif result.get("status") == "conflicts":
                print("Conflicts detected:")
                for c in result.get("conflicts", []):
                    print(f"  - {c['table']} pk={c['pk_value']}: {c['conflict_type']}")
            break

        if user_input.lower() == "reject":
            session.reject()
            print("Session discarded.")
            break

        if user_input.lower() == "diff":
            diff = session.get_diff()
            if diff.get("tables"):
                for table, changes in diff["tables"].items():
                    print(f"\n{table}:")
                    for change in changes:
                        print(f"  {change}")
            else:
                print("No changes staged yet.")
            continue

        try:
            result = agent.invoke({"input": user_input})
            print(f"\nAgent: {result['output']}\n")
        except Exception as e:
            print(f"\nError: {e}\n")


if __name__ == "__main__":
    interactive_session()
