"""Multi-turn chatbot with Astapa database tools."""

import json
import os
import re
from openai import OpenAI
from astapa import Astapa

openai = OpenAI()
astapa = Astapa(
    api_url=os.getenv("ASTAPA_API_URL", "http://localhost:8080"),
    proxy_host=os.getenv("ASTAPA_PROXY_HOST", "localhost"),
    proxy_port=int(os.getenv("ASTAPA_PROXY_PORT", "5433")),
)

tools = [
    {
        "type": "function",
        "function": {
            "name": "query",
            "description": "Execute a SELECT query on the database",
            "parameters": {
                "type": "object",
                "properties": {
                    "sql": {"type": "string", "description": "SQL SELECT query"}
                },
                "required": ["sql"]
            }
        }
    },
    {
        "type": "function",
        "function": {
            "name": "mutate",
            "description": "Execute UPDATE, INSERT, or DELETE. Changes are staged for review.",
            "parameters": {
                "type": "object",
                "properties": {
                    "sql": {"type": "string", "description": "SQL mutation query"}
                },
                "required": ["sql"]
            }
        }
    }
]

SELECT_PATTERN = re.compile(r"^\s*SELECT\s", re.IGNORECASE)
MUTATE_PATTERN = re.compile(r"^\s*(INSERT|UPDATE|DELETE)\s", re.IGNORECASE)
DANGEROUS_PATTERN = re.compile(r"(DROP|TRUNCATE|ALTER|CREATE|GRANT|REVOKE)", re.IGNORECASE)


def validate_sql(sql: str, expected_type: str) -> tuple[bool, str]:
    if not sql or not sql.strip():
        return False, "Empty SQL query"
    if DANGEROUS_PATTERN.search(sql):
        return False, "Query contains blocked keywords"
    if expected_type == "select" and not SELECT_PATTERN.match(sql):
        return False, "Query tool only accepts SELECT statements"
    if expected_type == "mutate" and not MUTATE_PATTERN.match(sql):
        return False, "Mutate tool only accepts INSERT, UPDATE, or DELETE statements"
    return True, ""


def run_chatbot():
    """Run a multi-turn chatbot with a persistent Astapa session."""

    required = ["DATABASE_NAME", "DATABASE_USER", "DATABASE_PASSWORD", "OPENAI_API_KEY"]
    missing = [k for k in required if not os.environ.get(k)]
    if missing:
        print(f"Missing environment variables: {', '.join(missing)}")
        return

    session = astapa.create_session(project_id="chatbot")
    session.connect(
        database=os.environ["DATABASE_NAME"],
        user=os.environ["DATABASE_USER"],
        password=os.environ["DATABASE_PASSWORD"],
    )

    def handle_tool(name: str, args: dict) -> str:
        try:
            sql = args.get("sql", "")
            if name == "query":
                valid, error = validate_sql(sql, "select")
                if not valid:
                    return f"Error: {error}"
                result = session.execute(sql)
                return json.dumps(result) if result else "No results"
            elif name == "mutate":
                valid, error = validate_sql(sql, "mutate")
                if not valid:
                    return f"Error: {error}"
                session.execute(sql)
                return "Mutation staged"
            return "Unknown tool"
        except Exception as e:
            return f"Error: {str(e)}"

    messages = [
        {"role": "system", "content": "You are a database assistant. Use tools to help the user query and modify data. All writes are staged for review."}
    ]

    print(f"Session {session.id} started. Type 'done' to propose changes, 'quit' to discard.\n")

    while True:
        user_input = input("You: ").strip()
        if not user_input:
            continue

        if user_input.lower() == "quit":
            print("Session discarded.")
            break

        if user_input.lower() == "done":
            diff = session.diff()
            if diff:
                print(f"\nStaged changes:\n{json.dumps(diff, indent=2)}")
                session.propose()
                print(f"\nSession {session.id} proposed for review.")
            else:
                print("No changes to propose.")
            break

        messages.append({"role": "user", "content": user_input})

        # Agent turn (may involve multiple tool calls)
        while True:
            try:
                response = openai.chat.completions.create(
                    model="gpt-4",
                    messages=messages,
                    tools=tools,
                )
            except Exception as e:
                print(f"API error: {e}")
                break

            msg = response.choices[0].message
            messages.append(msg)

            if not msg.tool_calls:
                print(f"Agent: {msg.content}\n")
                break

            for tc in msg.tool_calls:
                try:
                    args = json.loads(tc.function.arguments)
                except json.JSONDecodeError:
                    args = {}
                result = handle_tool(tc.function.name, args)
                messages.append({
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "content": result
                })


if __name__ == "__main__":
    run_chatbot()
