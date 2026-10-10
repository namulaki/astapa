/**
 * Astapa SDK Types
 */

/** Configuration for the Astapa client */
export interface AstapaConfig {
  /** Base URL for the astapa API (e.g., 'http://localhost:8080') */
  apiUrl: string;
  /** Hostname of the PostgreSQL proxy (default: 'localhost') */
  proxyHost?: string;
  /** Port of the PostgreSQL proxy (default: 5433) */
  proxyPort?: number;
}

/** Options for creating a new session */
export interface CreateSessionOptions {
  /** Project identifier for the session */
  projectId: string;
  /** Proxy mode: 'wiper' (discard on end) or 'agentic' (review required) */
  mode?: 'wiper' | 'agentic';
}

/** Session status */
export type SessionStatus = 'active' | 'proposed' | 'approved' | 'rejected' | 'destroyed';

/** Session details returned from the API */
export interface SessionDetails {
  id: string;
  schema_name: string;
  status: SessionStatus;
  project_id: string;
  created_at: string;
  updated_at?: string;
}

/** PostgreSQL connection configuration */
export interface ConnectionConfig {
  host: string;
  port: number;
  database: string;
  user: string;
  password: string;
  options: string;
}

/** Base connection options (without astapa-specific settings) */
export interface BaseConnectionOptions {
  database: string;
  user: string;
  password: string;
}

/** Table diff entry */
export interface TableDiff {
  table_name: string;
  inserts: number;
  updates: number;
  deletes: number;
  rows?: DiffRow[];
}

/** Individual row diff */
export interface DiffRow {
  operation: 'INSERT' | 'UPDATE' | 'DELETE';
  primary_key: Record<string, unknown>;
  old_values?: Record<string, unknown>;
  new_values?: Record<string, unknown>;
}

/** Diff response */
export interface DiffResponse {
  session_id: string;
  tables: TableDiff[];
}

/** Mutation log entry */
export interface Mutation {
  id: string;
  timestamp: string;
  operation: string;
  table_name: string;
  row_data: Record<string, unknown>;
}

/** Mutations response */
export interface MutationsResponse {
  session_id: string;
  mutations: Mutation[];
}

/** API error response */
export interface ApiError {
  error: string;
  message?: string;
}

/** Generic API response wrapper */
export interface ApiResponse<T> {
  ok: boolean;
  data?: T;
  error?: string;
}
