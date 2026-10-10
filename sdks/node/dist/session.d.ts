import { Pool } from 'pg';
import type { SessionDetails, ConnectionConfig, BaseConnectionOptions, DiffResponse, MutationsResponse } from './types.js';
/**
 * Represents an active astapa session.
 * Provides methods to interact with the session and create PostgreSQL connections.
 */
export declare class Session {
    private readonly apiUrl;
    private readonly proxyHost;
    private readonly proxyPort;
    private readonly details;
    private pools;
    constructor(apiUrl: string, proxyHost: string, proxyPort: number, details: SessionDetails);
    /** Session ID */
    get id(): string;
    /** Schema name used by this session */
    get schemaName(): string;
    /** Current session status */
    get status(): string;
    /** Project ID associated with this session */
    get projectId(): string;
    /**
     * Get the full session details
     */
    getDetails(): SessionDetails;
    /**
     * Refresh session details from the server
     */
    refresh(): Promise<SessionDetails>;
    /**
     * Get PostgreSQL connection configuration for this session.
     * The session ID is passed via the options parameter.
     */
    connectionConfig(options: BaseConnectionOptions): ConnectionConfig;
    /**
     * Create a pg Pool configured for this session.
     * The pool automatically includes the session context in all connections.
     */
    createPool(options: BaseConnectionOptions): Pool;
    /**
     * Get the diff of changes made in this session, grouped by table.
     */
    diff(): Promise<DiffResponse>;
    /**
     * Get the chronological log of mutations in this session.
     */
    mutations(): Promise<MutationsResponse>;
    /**
     * Submit the session for review.
     */
    propose(): Promise<SessionDetails>;
    /**
     * Approve and apply the changes from this session.
     */
    approve(): Promise<SessionDetails>;
    /**
     * Reject and discard the changes from this session.
     */
    reject(): Promise<SessionDetails>;
    /**
     * Destroy the session and clean up resources.
     * This will also end all pools created by this session.
     */
    destroy(): Promise<void>;
}
//# sourceMappingURL=session.d.ts.map