import { Pool, PoolConfig } from 'pg';
import type {
  SessionDetails,
  ConnectionConfig,
  BaseConnectionOptions,
  DiffResponse,
  MutationsResponse,
} from './types.js';

/** Helper type for error responses */
interface ErrorBody {
  error?: string;
}

/** Parse error body from response */
async function parseErrorBody(response: Response): Promise<ErrorBody> {
  return response.json().catch(() => ({ error: 'Unknown error' })) as Promise<ErrorBody>;
}

/**
 * Represents an active astapa session.
 * Provides methods to interact with the session and create PostgreSQL connections.
 */
export class Session {
  private readonly apiUrl: string;
  private readonly proxyHost: string;
  private readonly proxyPort: number;
  private readonly details: SessionDetails;
  private pools: Pool[] = [];

  constructor(
    apiUrl: string,
    proxyHost: string,
    proxyPort: number,
    details: SessionDetails
  ) {
    this.apiUrl = apiUrl;
    this.proxyHost = proxyHost;
    this.proxyPort = proxyPort;
    this.details = details;
  }

  /** Session ID */
  get id(): string {
    return this.details.id;
  }

  /** Schema name used by this session */
  get schemaName(): string {
    return this.details.schema_name;
  }

  /** Current session status */
  get status(): string {
    return this.details.status;
  }

  /** Project ID associated with this session */
  get projectId(): string {
    return this.details.project_id;
  }

  /**
   * Get the full session details
   */
  getDetails(): SessionDetails {
    return { ...this.details };
  }

  /**
   * Refresh session details from the server
   */
  async refresh(): Promise<SessionDetails> {
    const response = await fetch(`${this.apiUrl}/sessions/${this.id}`);

    if (!response.ok) {
      const body = await parseErrorBody(response);
      throw new Error(`Failed to refresh session: ${body.error || response.statusText}`);
    }

    const updated = await response.json() as SessionDetails;
    Object.assign(this.details, updated);
    return this.getDetails();
  }

  /**
   * Get PostgreSQL connection configuration for this session.
   * The session ID is passed via the options parameter.
   */
  connectionConfig(options: BaseConnectionOptions): ConnectionConfig {
    return {
      host: this.proxyHost,
      port: this.proxyPort,
      database: options.database,
      user: options.user,
      password: options.password,
      options: `-c astapa_session=${this.id}`,
    };
  }

  /**
   * Create a pg Pool configured for this session.
   * The pool automatically includes the session context in all connections.
   */
  createPool(options: BaseConnectionOptions): Pool {
    const config: PoolConfig = {
      host: this.proxyHost,
      port: this.proxyPort,
      database: options.database,
      user: options.user,
      password: options.password,
      options: `-c astapa_session=${this.id}`,
    };

    const pool = new Pool(config);
    this.pools.push(pool);
    return pool;
  }

  /**
   * Get the diff of changes made in this session, grouped by table.
   */
  async diff(): Promise<DiffResponse> {
    const response = await fetch(`${this.apiUrl}/sessions/${this.id}/diff`);

    if (!response.ok) {
      const body = await parseErrorBody(response);
      throw new Error(`Failed to get diff: ${body.error || response.statusText}`);
    }

    return response.json() as Promise<DiffResponse>;
  }

  /**
   * Get the chronological log of mutations in this session.
   */
  async mutations(): Promise<MutationsResponse> {
    const response = await fetch(`${this.apiUrl}/sessions/${this.id}/mutations`);

    if (!response.ok) {
      const body = await parseErrorBody(response);
      throw new Error(`Failed to get mutations: ${body.error || response.statusText}`);
    }

    return response.json() as Promise<MutationsResponse>;
  }

  /**
   * Submit the session for review.
   */
  async propose(): Promise<SessionDetails> {
    const response = await fetch(`${this.apiUrl}/sessions/${this.id}/propose`, {
      method: 'POST',
    });

    if (!response.ok) {
      const body = await parseErrorBody(response);
      throw new Error(`Failed to propose session: ${body.error || response.statusText}`);
    }

    const updated = await response.json() as SessionDetails;
    Object.assign(this.details, updated);
    return this.getDetails();
  }

  /**
   * Approve and apply the changes from this session.
   */
  async approve(): Promise<SessionDetails> {
    const response = await fetch(`${this.apiUrl}/sessions/${this.id}/approve`, {
      method: 'POST',
    });

    if (!response.ok) {
      const body = await parseErrorBody(response);
      throw new Error(`Failed to approve session: ${body.error || response.statusText}`);
    }

    const updated = await response.json() as SessionDetails;
    Object.assign(this.details, updated);
    return this.getDetails();
  }

  /**
   * Reject and discard the changes from this session.
   */
  async reject(): Promise<SessionDetails> {
    const response = await fetch(`${this.apiUrl}/sessions/${this.id}/reject`, {
      method: 'POST',
    });

    if (!response.ok) {
      const body = await parseErrorBody(response);
      throw new Error(`Failed to reject session: ${body.error || response.statusText}`);
    }

    const updated = await response.json() as SessionDetails;
    Object.assign(this.details, updated);
    return this.getDetails();
  }

  /**
   * Destroy the session and clean up resources.
   * This will also end all pools created by this session.
   */
  async destroy(): Promise<void> {
    // End all pools first
    await Promise.all(this.pools.map(pool => pool.end()));
    this.pools = [];

    const response = await fetch(`${this.apiUrl}/sessions/${this.id}`, {
      method: 'DELETE',
    });

    if (!response.ok) {
      const body = await parseErrorBody(response);
      throw new Error(`Failed to destroy session: ${body.error || response.statusText}`);
    }

    Object.assign(this.details, { status: 'destroyed' });
  }
}
