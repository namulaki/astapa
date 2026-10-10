# Astapa MCP Server

MCP (Model Context Protocol) server for Claude Code integration.

## Install

```bash
npm install -g @astapa/mcp-server
```

## Configure Claude Code

Add to your MCP settings (`~/.claude/settings.json`):

```json
{
  "mcpServers": {
    "astapa": {
      "command": "astapa-mcp",
      "env": {
        "ASTAPA_API_URL": "http://localhost:8080",
        "ASTAPA_PROXY_HOST": "localhost",
        "ASTAPA_PROXY_PORT": "5433",
        "DATABASE_NAME": "mydb",
        "DATABASE_USER": "postgres",
        "DATABASE_PASSWORD": "pass"
      }
    }
  }
}
```

## Tools

| Tool | Description |
|------|-------------|
| `astapa_query` | Execute SELECT queries |
| `astapa_mutate` | Execute INSERT/UPDATE/DELETE (staged) |
| `astapa_diff` | View pending changes |
| `astapa_propose` | Submit for human review |

## Usage

Once configured, Claude Code can use the tools:

```
You: Find users who haven't logged in for 30 days and mark them inactive

Claude: [astapa_query] SELECT id, email FROM users WHERE last_login < NOW() - INTERVAL '30 days'
        → Found 42 users

        [astapa_mutate] UPDATE users SET status = 'inactive' WHERE last_login < NOW() - INTERVAL '30 days'
        → Staged 42 updates

        [astapa_diff]
        → users: 42 rows modified

        Ready to propose when you want to submit for review.
```
