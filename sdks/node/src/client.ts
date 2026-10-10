import { Session } from './session.js';
import type { AstapaConfig, CreateSessionOptions, SessionDetails } from './types.js';

/**
 * Main client for interacting with the astapa service.
 */
export class Astapa {
  private readonly apiUrl: string;
  private readonly proxyHost: string;
  private readonly proxyPort: number;

  /**
   * Create a new Astapa client.
   *
   * @param config - Client configuration
   * @param config.apiUrl - Base URL for the astapa API
   * @param config.proxyHost - Hostname of the PostgreSQL proxy (default: 'localhost')
   * @param config.proxyPort - Port of the PostgreSQL proxy (default: 5433)
   */
  constructor(config: AstapaConfig) {
    this.apiUrl = config.apiUrl.replace(/\/$/, ''); // Remove trailing slash
    this.proxyHost = config.proxyHost ?? 'localhost';
    this.proxyPort = config.proxyPort ?? 5433;
  }

  /**
   * Create a new session for staging database changes.
   *
   * @param options - Session creation options
   * @param options.projectId - Project identifier for the session
   * @param options.mode - Proxy mode: 'wiper' or 'agentic' (default: 'agentic')
   * @returns A new Session instance
   */
  async createSession(options: CreateSessionOptions): Promise<Session> {
    const response = await fetch(`${this.apiUrl}/sessions`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        project_id: options.projectId,
        ...(options.mode && { mode: options.mode }),
      }),
    });

    if (!response.ok) {
      const body = await response.json().catch(() => ({ error: 'Unknown error' })) as { error?: string };
      throw new Error(`Failed to create session: ${body.error || response.statusText}`);
    }

    const details = await response.json() as SessionDetails;
    return new Session(this.apiUrl, this.proxyHost, this.proxyPort, details);
  }

  /**
   * Get an existing session by ID.
   *
   * @param sessionId - The session ID to retrieve
   * @returns The Session instance
   */
  async getSession(sessionId: string): Promise<Session> {
    const response = await fetch(`${this.apiUrl}/sessions/${sessionId}`);

    if (!response.ok) {
      const body = await response.json().catch(() => ({ error: 'Unknown error' })) as { error?: string };
      throw new Error(`Failed to get session: ${body.error || response.statusText}`);
    }

    const details = await response.json() as SessionDetails;
    return new Session(this.apiUrl, this.proxyHost, this.proxyPort, details);
  }
}
