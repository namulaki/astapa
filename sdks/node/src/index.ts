/**
 * Astapa Node.js SDK
 *
 * A TypeScript SDK for interacting with astapa,
 * a PostgreSQL change staging and review system.
 */

export { Astapa } from './client.js';
export { Session } from './session.js';
export type {
  AstapaConfig,
  CreateSessionOptions,
  SessionDetails,
  SessionStatus,
  ConnectionConfig,
  BaseConnectionOptions,
  DiffResponse,
  TableDiff,
  DiffRow,
  MutationsResponse,
  Mutation,
  ApiError,
  ApiResponse,
} from './types.js';
