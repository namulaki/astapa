"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.Session = void 0;
const pg_1 = require("pg");
/** Parse error body from response */
async function parseErrorBody(response) {
    return response.json().catch(() => ({ error: 'Unknown error' }));
}
/**
 * Represents an active astapa session.
 * Provides methods to interact with the session and create PostgreSQL connections.
 */
class Session {
    apiUrl;
    proxyHost;
    proxyPort;
    details;
    pools = [];
    constructor(apiUrl, proxyHost, proxyPort, details) {
        this.apiUrl = apiUrl;
        this.proxyHost = proxyHost;
        this.proxyPort = proxyPort;
        this.details = details;
    }
    /** Session ID */
    get id() {
        return this.details.id;
    }
    /** Schema name used by this session */
    get schemaName() {
        return this.details.schema_name;
    }
    /** Current session status */
    get status() {
        return this.details.status;
    }
    /** Project ID associated with this session */
    get projectId() {
        return this.details.project_id;
    }
    /**
     * Get the full session details
     */
    getDetails() {
        return { ...this.details };
    }
    /**
     * Refresh session details from the server
     */
    async refresh() {
        const response = await fetch(`${this.apiUrl}/sessions/${this.id}`);
        if (!response.ok) {
            const body = await parseErrorBody(response);
            throw new Error(`Failed to refresh session: ${body.error || response.statusText}`);
        }
        const updated = await response.json();
        Object.assign(this.details, updated);
        return this.getDetails();
    }
    /**
     * Get PostgreSQL connection configuration for this session.
     * The session ID is passed via the options parameter.
     */
    connectionConfig(options) {
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
    createPool(options) {
        const config = {
            host: this.proxyHost,
            port: this.proxyPort,
            database: options.database,
            user: options.user,
            password: options.password,
            options: `-c astapa_session=${this.id}`,
        };
        const pool = new pg_1.Pool(config);
        this.pools.push(pool);
        return pool;
    }
    /**
     * Get the diff of changes made in this session, grouped by table.
     */
    async diff() {
        const response = await fetch(`${this.apiUrl}/sessions/${this.id}/diff`);
        if (!response.ok) {
            const body = await parseErrorBody(response);
            throw new Error(`Failed to get diff: ${body.error || response.statusText}`);
        }
        return response.json();
    }
    /**
     * Get the chronological log of mutations in this session.
     */
    async mutations() {
        const response = await fetch(`${this.apiUrl}/sessions/${this.id}/mutations`);
        if (!response.ok) {
            const body = await parseErrorBody(response);
            throw new Error(`Failed to get mutations: ${body.error || response.statusText}`);
        }
        return response.json();
    }
    /**
     * Submit the session for review.
     */
    async propose() {
        const response = await fetch(`${this.apiUrl}/sessions/${this.id}/propose`, {
            method: 'POST',
        });
        if (!response.ok) {
            const body = await parseErrorBody(response);
            throw new Error(`Failed to propose session: ${body.error || response.statusText}`);
        }
        const updated = await response.json();
        Object.assign(this.details, updated);
        return this.getDetails();
    }
    /**
     * Approve and apply the changes from this session.
     */
    async approve() {
        const response = await fetch(`${this.apiUrl}/sessions/${this.id}/approve`, {
            method: 'POST',
        });
        if (!response.ok) {
            const body = await parseErrorBody(response);
            throw new Error(`Failed to approve session: ${body.error || response.statusText}`);
        }
        const updated = await response.json();
        Object.assign(this.details, updated);
        return this.getDetails();
    }
    /**
     * Reject and discard the changes from this session.
     */
    async reject() {
        const response = await fetch(`${this.apiUrl}/sessions/${this.id}/reject`, {
            method: 'POST',
        });
        if (!response.ok) {
            const body = await parseErrorBody(response);
            throw new Error(`Failed to reject session: ${body.error || response.statusText}`);
        }
        const updated = await response.json();
        Object.assign(this.details, updated);
        return this.getDetails();
    }
    /**
     * Destroy the session and clean up resources.
     * This will also end all pools created by this session.
     */
    async destroy() {
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
exports.Session = Session;
//# sourceMappingURL=session.js.map