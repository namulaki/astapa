import { Session } from './session.js';
import type { AstapaConfig, CreateSessionOptions } from './types.js';
/**
 * Main client for interacting with the astapa service.
 */
export declare class Astapa {
    private readonly apiUrl;
    private readonly proxyHost;
    private readonly proxyPort;
    /**
     * Create a new Astapa client.
     *
     * @param config - Client configuration
     * @param config.apiUrl - Base URL for the astapa API
     * @param config.proxyHost - Hostname of the PostgreSQL proxy (default: 'localhost')
     * @param config.proxyPort - Port of the PostgreSQL proxy (default: 5433)
     */
    constructor(config: AstapaConfig);
    /**
     * Create a new session for staging database changes.
     *
     * @param options - Session creation options
     * @param options.projectId - Project identifier for the session
     * @param options.mode - Proxy mode: 'wiper' or 'agentic' (default: 'agentic')
     * @returns A new Session instance
     */
    createSession(options: CreateSessionOptions): Promise<Session>;
    /**
     * Get an existing session by ID.
     *
     * @param sessionId - The session ID to retrieve
     * @returns The Session instance
     */
    getSession(sessionId: string): Promise<Session>;
}
//# sourceMappingURL=client.d.ts.map