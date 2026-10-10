#!/usr/bin/env node

import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import { Astapa, Session } from "astapa";
import { Pool } from "pg";

// Proxy modes
type Mode = "wiper" | "agentic";

function parseMode(value: string | undefined): Mode {
  const normalized = value?.toLowerCase();
  if (normalized === "wiper" || normalized === "agentic") {
    return normalized;
  }
  if (normalized !== undefined) {
    console.error("Error: ASTAPA_MODE must be 'wiper' or 'agentic'");
    process.exit(1);
  }
  return "agentic"; // default
}

const API_URL = process.env.ASTAPA_API_URL || "http://localhost:8080";
const PROXY_HOST = process.env.ASTAPA_PROXY_HOST || "localhost";
const PROXY_PORT = parseInt(process.env.ASTAPA_PROXY_PORT || "5433");
const DATABASE = process.env.DATABASE_NAME;
const USER = process.env.DATABASE_USER;
const PASSWORD = process.env.DATABASE_PASSWORD;
const MODE: Mode = parseMode(process.env.ASTAPA_MODE);

if (!DATABASE || !USER || !PASSWORD) {
  console.error("Error: DATABASE_NAME, DATABASE_USER, and DATABASE_PASSWORD environment variables are required");
  process.exit(1);
}

const MODE_DESCRIPTIONS: Record<Mode, string> = {
  wiper: "WIPER MODE: All mutations are staged in a sandbox and discarded at session end. Safe for testing.",
  agentic: "AGENTIC MODE: All mutations are staged for human review. Use astapa_propose to submit for approval.",
};

const client = new Astapa({
  apiUrl: API_URL,
  proxyHost: PROXY_HOST,
  proxyPort: PROXY_PORT,
});

let session: Session | null = null;
let pool: Pool | null = null;

function classifySQL(sql: string): "select" | "mutate" | "other" {
  const normalized = sql
    .replace(/\/\*[\s\S]*?\*\//g, "")  // remove block comments
    .replace(/--.*$/gm, "")             // remove line comments
    .trim()
    .toUpperCase();

  // Remove string literals before checking for semicolons
  const withoutStrings = normalized.replace(/'(?:[^']|'')*'/g, "");
  const semicolonIndex = withoutStrings.indexOf(";");
  if (semicolonIndex !== -1 && semicolonIndex < withoutStrings.length - 1) {
    return "other";
  }

  if (normalized.startsWith("SELECT") || normalized.startsWith("WITH")) {
    return "select";
  }
  if (normalized.startsWith("INSERT") || normalized.startsWith("UPDATE") || normalized.startsWith("DELETE")) {
    return "mutate";
  }
  return "other";
}

async function cleanup() {
  const currentSession = session;
  session = null;
  pool = null;

  if (currentSession) {
    try {
      await currentSession.destroy();
    } catch (e) {
      console.error("Failed to destroy session:", e);
    }
  }
}

process.on("SIGINT", async () => {
  await cleanup();
  process.exit(0);
});

process.on("SIGTERM", async () => {
  await cleanup();
  process.exit(0);
});

const server = new Server(
  { name: "astapa", version: "0.1.0" },
  { capabilities: { tools: {} } }
);

server.setRequestHandler(ListToolsRequestSchema, async () => {
  const modeInfo = MODE_DESCRIPTIONS[MODE];

  const baseTools: Array<{
    name: string;
    description: string;
    inputSchema: {
      type: string;
      properties: Record<string, unknown>;
      required?: string[];
    };
  }> = [
    {
      name: "astapa_query",
      description: `Execute a SELECT query on the database. Returns query results.\n\n${modeInfo}`,
      inputSchema: {
        type: "object",
        properties: {
          sql: { type: "string", description: "SQL SELECT query" },
        },
        required: ["sql"],
      },
    },
    {
      name: "astapa_mutate",
      description: MODE === "wiper"
        ? `Execute INSERT, UPDATE, or DELETE. Changes are staged in sandbox and will be discarded at session end.\n\n${modeInfo}`
        : `Execute INSERT, UPDATE, or DELETE. Changes are staged for human review, not applied immediately.\n\n${modeInfo}`,
      inputSchema: {
        type: "object",
        properties: {
          sql: { type: "string", description: "SQL mutation (INSERT/UPDATE/DELETE)" },
        },
        required: ["sql"],
      },
    },
  ];

  // Both modes have diff tool
  baseTools.push({
    name: "astapa_diff",
    description: `View all pending changes staged in the current session.\n\n${modeInfo}`,
    inputSchema: {
      type: "object",
      properties: {},
    },
  });

  // Only include propose tool in agentic mode
  if (MODE === "agentic") {
    baseTools.push({
      name: "astapa_propose",
      description: `Submit all staged changes for human review. After proposing, no more changes can be made.\n\n${modeInfo}`,
      inputSchema: {
        type: "object",
        properties: {},
      },
    });
  }

  // Include discard tool in wiper mode
  if (MODE === "wiper") {
    baseTools.push({
      name: "astapa_discard",
      description: `Discard all staged changes and reset the session. Use when you want to start fresh.\n\n${modeInfo}`,
      inputSchema: {
        type: "object",
        properties: {},
      },
    });
  }

  return { tools: baseTools };
});

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;

  try {
    // Ensure session and pool exist
    if (!session || !pool) {
      session = await client.createSession({ projectId: "claude-code", mode: MODE });
      pool = session.createPool({
        database: DATABASE,
        user: USER,
        password: PASSWORD,
      });
    }

    switch (name) {
      case "astapa_query": {
        const sql = (args as { sql: string }).sql;
        if (classifySQL(sql) !== "select") {
          return { content: [{ type: "text", text: "Error: Only SELECT queries allowed. Use astapa_mutate for INSERT/UPDATE/DELETE." }] };
        }
        const result = await pool.query(sql);
        return { content: [{ type: "text", text: JSON.stringify(result.rows, null, 2) }] };
      }

      case "astapa_mutate": {
        const sql = (args as { sql: string }).sql;
        if (classifySQL(sql) !== "mutate") {
          return { content: [{ type: "text", text: "Error: Only INSERT/UPDATE/DELETE allowed. Use astapa_query for SELECT." }] };
        }
        const result = await pool.query(sql);

        if (MODE === "wiper") {
          return { content: [{ type: "text", text: `Mutation staged (will be discarded at session end): ${result.rowCount} row(s) affected` }] };
        } else {
          return { content: [{ type: "text", text: `Mutation staged for review: ${result.rowCount} row(s) affected` }] };
        }
      }

      case "astapa_diff": {
        if (!session) {
          return { content: [{ type: "text", text: "No active session. Run a query or mutation first." }] };
        }
        const diff = await session.diff();
        if (!diff || Object.keys(diff).length === 0) {
          return { content: [{ type: "text", text: "No pending changes" }] };
        }
        return { content: [{ type: "text", text: JSON.stringify(diff, null, 2) }] };
      }

      case "astapa_propose": {
        if (MODE !== "agentic") {
          return { content: [{ type: "text", text: `Error: Propose is only available in agentic mode. Current mode: ${MODE}` }] };
        }
        if (!session) {
          return { content: [{ type: "text", text: "No active session. Run a mutation first." }] };
        }
        const sessionId = session.id;
        try {
          await session.propose();
        } finally {
          session = null;
          pool = null;
        }
        return { content: [{ type: "text", text: `Session ${sessionId} proposed for review. Session closed.` }] };
      }

      case "astapa_discard": {
        if (MODE !== "wiper") {
          return { content: [{ type: "text", text: `Error: Discard is only available in wiper mode. Current mode: ${MODE}` }] };
        }
        if (!session) {
          return { content: [{ type: "text", text: "No active session to discard." }] };
        }
        const sessionId = session.id;
        await cleanup();
        return { content: [{ type: "text", text: `Session ${sessionId} discarded. All staged changes removed.` }] };
      }

      default:
        return { content: [{ type: "text", text: `Unknown tool: ${name}` }] };
    }
  } catch (error: any) {
    return { content: [{ type: "text", text: `Error: ${error.message}` }] };
  }
});

async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
  console.error(`Astapa MCP server running in ${MODE.toUpperCase()} mode`);
}

main().catch(console.error);
