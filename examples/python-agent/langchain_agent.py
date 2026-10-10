"""LangChain agent with Astapa database tools."""

import json
import os
import re
from langchain.tools import tool
from langchain_openai import ChatOpenAI
from langchain.agents import create_openai_tools_agent, AgentExecutor
from langchain_core.prompts import ChatPromptTemplate
from astapa import Astapa

astapa = Astapa(
    api_url=os.getenv("ASTAPA_API_URL", "http://localhost:8080"),
    proxy_host=os.getenv("ASTAPA_PROXY_HOST", "localhost"),
    proxy_port=int(os.getenv("ASTAPA_PROXY_PORT", "5433")),
)
session = None  # Set when running

# SQL validation patterns
SELECT_PATTERN = re.compile(r"^\s*SELECT\s", re.IGNORECASE)
MUTATE_PATTERN = re.compile(r"^\s*(INSERT|UPDATE|DELETE)\s", re.IGNORECASE)
DANGEROUS_PATTERN = re.compile(r"(DROP|TRUNCATE|ALTER|CREATE|GRANT|REVOKE)", re.IGNORECASE)


def validate_sql(sql: str, expected_type: str) -> tuple[bool, str]:
    """Validate SQL query type and check for dangerous patterns."""
    if not sql or not sql.strip():
        return False, "Empty SQL query"

    if DANGEROUS_PATTERN.search(sql):
        return False, "Query contains blocked keywords (DROP, TRUNCATE, ALTER, etc.)"

    if expected_type == "select" and not SELECT_PATTERN.match(sql):
        return False, "Query tool only accepts SELECT statements"

    if expected_type == "mutate" and not MUTATE_PATTERN.match(sql):
        return False, "Mutate tool only accepts INSERT, UPDATE, or DELETE statements"

    return True, ""


@tool
def query(sql: str) -> str:
    """Execute a SELECT query on the database."""
    if session is None:
        return "Error: No active session"
    try:
        valid, error = validate_sql(sql, "select")
        if not valid:
            return f"Error: {error}"
        result = session.execute(sql)
        return json.dumps(result) if result else "No results"
    except Exception as e:
        return f"Error: {str(e)}"


@tool
def mutate(sql: str) -> str:
    """Execute UPDATE, INSERT, or DELETE. Changes are staged for review."""
    if session is None:
        return "Error: No active session"
    try:
        valid, error = validate_sql(sql, "mutate")
        if not valid:
            return f"Error: {error}"
        session.execute(sql)
        return "Mutation staged for review"
    except Exception as e:
        return f"Error: {str(e)}"


def run_agent(user_input: str):
    """Run the LangChain agent with a Astapa session."""
    global session

    required = ["DATABASE_NAME", "DATABASE_USER", "DATABASE_PASSWORD", "OPENAI_API_KEY"]
    missing = [k for k in required if not os.environ.get(k)]
    if missing:
        print(f"Missing environment variables: {', '.join(missing)}")
        return None

    llm = ChatOpenAI(model="gpt-4")
    tools = [query, mutate]

    prompt = ChatPromptTemplate.from_messages([
        ("system", "You are a database assistant. Use the tools to help the user."),
        ("human", "{input}"),
        ("placeholder", "{agent_scratchpad}"),
    ])

    agent = create_openai_tools_agent(llm, tools, prompt)
    executor = AgentExecutor(agent=agent, tools=tools, verbose=True)

    with astapa.session(
        project_id="langchain-agent",
        database=os.environ["DATABASE_NAME"],
        user=os.environ["DATABASE_USER"],
        password=os.environ["DATABASE_PASSWORD"],
    ) as sess:
        session = sess

        try:
            result = executor.invoke({"input": user_input})
            print(f"\nAgent: {result['output']}")
        except Exception as e:
            print(f"Agent error: {e}")
            return None

        # Show staged changes
        diff = sess.diff()
        if diff:
            print(f"\nStaged changes: {json.dumps(diff, indent=2)}")

        # Session auto-proposes on exit
        print(f"Session {sess.id} ready for review")
        return sess.id


if __name__ == "__main__":
    import sys
    prompt = sys.argv[1] if len(sys.argv) > 1 else "Show me all users"
    run_agent(prompt)
