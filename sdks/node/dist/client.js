"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.Astapa = void 0;
const session_js_1 = require("./session.js");
/**
 * Main client for interacting with the astapa service.
 */
class Astapa {
    apiUrl;
    proxyHost;
    proxyPort;
    /**
     * Create a new Astapa client.
     *
     * @param config - Client configuration
     * @param config.apiUrl - Base URL for the astapa API
     * @param config.proxyHost - Hostname of the PostgreSQL proxy (default: 'localhost')
     * @param config.proxyPort - Port of the PostgreSQL proxy (default: 5433)
     */
    constructor(config) {
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
    async createSession(options) {
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
            const body = await response.json().catch(() => ({ error: 'Unknown error' }));
            throw new Error(`Failed to create session: ${body.error || response.statusText}`);
        }
        const details = await response.json();
        return new session_js_1.Session(this.apiUrl, this.proxyHost, this.proxyPort, details);
    }
    /**
     * Get an existing session by ID.
     *
     * @param sessionId - The session ID to retrieve
     * @returns The Session instance
     */
    async getSession(sessionId) {
        const response = await fetch(`${this.apiUrl}/sessions/${sessionId}`);
        if (!response.ok) {
            const body = await response.json().catch(() => ({ error: 'Unknown error' }));
            throw new Error(`Failed to get session: ${body.error || response.statusText}`);
        }
        const details = await response.json();
        return new session_js_1.Session(this.apiUrl, this.proxyHost, this.proxyPort, details);
    }
}
exports.Astapa = Astapa;
//# sourceMappingURL=client.js.map