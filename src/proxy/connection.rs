#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::manual_strip)]
//! Per-connection state machine for Postgres wire protocol proxy.

use bytes::BytesMut;
use deadpool_postgres::Pool;
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::metrics::{QUERY_CACHE_HITS, QUERY_CACHE_MISSES};
use crate::proxy::protocol::{
    self, encode_backend_message, encode_error, encode_parse, encode_query, encode_startup,
    parse_backend_message, parse_frontend_message, BackendMessage, FrontendMessage, StartupMessage,
};
use crate::rewriter::{
    detect_point_lookup, DirtyKeyTracker, QueryAnalysis, QueryCache, QueryType, Rewriter,
};
use crate::session::schema::SchemaManager;
use crate::session::{SessionManager, SessionMode};

/// Default buffer size for connection I/O (16KB for better throughput).
const DEFAULT_BUFFER_SIZE: usize = 16384;

/// Connection state in the proxy lifecycle
#[derive(Debug, Clone, PartialEq)]
enum ConnectionState {
    /// Waiting for initial message (SSL or Startup)
    Initial,
    /// Waiting for startup message after SSL rejection
    AwaitingStartup,
    /// Authenticating with backend
    Authenticating,
    /// Ready for queries
    Ready,
    /// Connection terminated
    Terminated,
}

/// Per-connection handler
pub struct Connection {
    /// Client TCP stream
    client: TcpStream,
    /// Backend Postgres connection (established after startup)
    backend: Option<TcpStream>,
    /// Backend address to connect to
    backend_addr: String,
    /// Database pool for infrastructure operations
    pool: Arc<Pool>,
    /// Shared query cache
    query_cache: Arc<QueryCache>,
    /// Schema manager for creating views/shadow tables
    schema_manager: SchemaManager,
    /// Session manager for wiper cleanup
    session_manager: Option<Arc<SessionManager>>,
    /// Session ID extracted from astapa_session parameter
    session_id: Option<Uuid>,
    /// Session mode (wiper or agentic)
    session_mode: Option<SessionMode>,
    /// Schema name for this session (will be resolved from session_id)
    schema_name: Option<String>,
    /// SQL rewriter for this session
    rewriter: Option<Rewriter>,
    /// Read buffer for client messages
    client_buffer: BytesMut,
    /// Read buffer for backend messages
    backend_buffer: BytesMut,
    /// Current connection state
    state: ConnectionState,
    /// Original startup parameters from client
    startup_params: HashMap<String, String>,
    /// Tables with infrastructure already ensured (to avoid repeated checks)
    ensured_tables: HashMap<String, EnsuredInfra>,
    /// Tracks dirty (modified/deleted) keys per table for point lookup optimization
    dirty_tracker: DirtyKeyTracker,
    /// Cached primary key column names per table (table_name -> pk_column)
    pk_cache: HashMap<String, String>,
}

/// Tracks what infrastructure has been ensured for a table
#[derive(Default, Clone)]
struct EnsuredInfra {
    view: bool,
    shadow: bool,
    deleted: bool,
}

impl Connection {
    /// Create a new connection handler
    pub fn new(
        client: TcpStream,
        backend_addr: String,
        pool: Arc<Pool>,
        query_cache: Arc<QueryCache>,
        session_manager: Option<Arc<SessionManager>>,
    ) -> Self {
        let schema_manager = SchemaManager::new((*pool).clone());
        Self {
            client,
            backend: None,
            backend_addr,
            pool,
            query_cache,
            schema_manager,
            session_manager,
            session_id: None,
            session_mode: None,
            schema_name: None,
            rewriter: None,
            client_buffer: BytesMut::with_capacity(DEFAULT_BUFFER_SIZE),
            backend_buffer: BytesMut::with_capacity(DEFAULT_BUFFER_SIZE),
            state: ConnectionState::Initial,
            startup_params: HashMap::new(),
            ensured_tables: HashMap::new(),
            dirty_tracker: DirtyKeyTracker::new(),
            pk_cache: HashMap::new(),
        }
    }

    /// Run the connection handler
    pub async fn run(mut self) -> anyhow::Result<()> {
        let peer_addr = self.client.peer_addr().ok();
        info!(?peer_addr, "New connection");

        let result = self.handle_connection().await;

        if let Err(ref e) = result {
            // Don't log connection reset as error
            if !is_connection_closed_error(e) {
                error!(?peer_addr, error = %e, "Connection error");
            }
        }

        // Wiper mode: auto-destroy session on disconnect
        if self.session_mode == Some(SessionMode::Wiper) {
            if let (Some(session_id), Some(ref session_manager)) =
                (self.session_id, &self.session_manager)
            {
                info!(session_id = %session_id, "Wiper mode: destroying session on disconnect");
                if let Err(e) = session_manager.destroy(session_id).await {
                    warn!(session_id = %session_id, error = %e, "Failed to destroy wiper session");
                }
            }
        }

        info!(?peer_addr, "Connection closed");
        Ok(())
    }

    async fn handle_connection(&mut self) -> anyhow::Result<()> {
        // Phase 1: Handle startup (SSL negotiation + startup message)
        self.handle_startup_phase().await?;

        // Phase 2: Connect to backend and relay auth
        self.connect_backend().await?;
        self.relay_authentication().await?;

        // Phase 3: Main query loop
        self.state = ConnectionState::Ready;
        self.main_loop().await
    }

    /// Handle the startup phase (SSL request and/or startup message)
    async fn handle_startup_phase(&mut self) -> anyhow::Result<()> {
        loop {
            // Read data from client
            let n = self.client.read_buf(&mut self.client_buffer).await?;
            if n == 0 {
                anyhow::bail!("Client disconnected during startup");
            }

            // Try to parse message
            while let Some(msg) = parse_frontend_message(&mut self.client_buffer)? {
                match msg {
                    FrontendMessage::SslRequest => {
                        debug!("Received SSL request, rejecting");
                        // Reject SSL with 'N' and flush to ensure client receives it
                        self.client.write_all(b"N").await?;
                        self.client.flush().await?;
                        self.state = ConnectionState::AwaitingStartup;
                    }
                    FrontendMessage::Startup(startup) => {
                        debug!(?startup.parameters, "Received startup message");
                        self.process_startup(startup)?;
                        return Ok(());
                    }
                    _ => {
                        anyhow::bail!("Unexpected message during startup: {:?}", msg);
                    }
                }
            }
        }
    }

    /// Process startup message and extract session info
    fn process_startup(&mut self, startup: StartupMessage) -> anyhow::Result<()> {
        self.startup_params = startup.parameters.clone();

        // Extract astapa_session - check direct parameter first
        let session_str = startup
            .parameters
            .get("astapa_session")
            .cloned()
            .or_else(|| {
                // Check inside options parameter: "-c astapa_session=uuid"
                startup
                    .parameters
                    .get("options")
                    .and_then(|opts| extract_option_value(opts, "astapa_session"))
            });

        // Extract astapa_mode - check direct parameter first
        let mode_str = startup.parameters.get("astapa_mode").cloned().or_else(|| {
            // Check inside options parameter: "-c astapa_mode=wiper"
            startup
                .parameters
                .get("options")
                .and_then(|opts| extract_option_value(opts, "astapa_mode"))
        });

        // Parse mode (defaults to Agentic if not specified)
        self.session_mode = mode_str
            .as_ref()
            .and_then(|s| SessionMode::from_str(s))
            .or(Some(SessionMode::Agentic));

        if let Some(session_str) = session_str {
            match Uuid::parse_str(&session_str) {
                Ok(uuid) => {
                    let schema = format!("session_{}", uuid.to_string().replace('-', "_"));
                    self.session_id = Some(uuid);
                    self.rewriter = Some(Rewriter::new(&schema));
                    self.schema_name = Some(schema.clone());
                    info!(
                        session_id = %uuid,
                        schema = %schema,
                        mode = ?self.session_mode,
                        "Session identified, rewriter initialized"
                    );
                }
                Err(e) => {
                    warn!(session = session_str, error = %e, "Invalid session UUID");
                }
            }
        }

        Ok(())
    }

    /// Connect to backend Postgres
    async fn connect_backend(&mut self) -> anyhow::Result<()> {
        debug!(addr = %self.backend_addr, "Connecting to backend");

        let backend = TcpStream::connect(&self.backend_addr).await?;
        self.backend = Some(backend);

        // Send startup message to backend (without astapa_session and astapa_mode params)
        let mut backend_params = self.startup_params.clone();
        backend_params.remove("astapa_session");
        backend_params.remove("astapa_mode");

        // Also strip astapa_session and astapa_mode from options parameter if present
        if let Some(opts) = backend_params.get("options").cloned() {
            let cleaned = strip_option_value(&opts, "astapa_session");
            let cleaned = strip_option_value(&cleaned, "astapa_mode");
            if cleaned.trim().is_empty() {
                backend_params.remove("options");
            } else {
                backend_params.insert("options".to_string(), cleaned);
            }
        }

        let startup_msg = encode_startup(&backend_params);
        self.backend
            .as_mut()
            .unwrap()
            .write_all(&startup_msg)
            .await?;

        self.state = ConnectionState::Authenticating;
        debug!("Backend connection established, starting auth");

        Ok(())
    }

    /// Relay authentication messages between client and backend
    async fn relay_authentication(&mut self) -> anyhow::Result<()> {
        let backend = self.backend.as_mut().unwrap();

        loop {
            // Read from backend
            let n = backend.read_buf(&mut self.backend_buffer).await?;
            if n == 0 {
                anyhow::bail!("Backend disconnected during authentication");
            }

            // Parse and relay messages
            while let Some(msg) = parse_backend_message(&mut self.backend_buffer)? {
                let encoded = encode_backend_message(&msg);

                match &msg {
                    BackendMessage::Authentication(payload) => {
                        // Check if this is AuthenticationOk (type = 0)
                        if payload.len() >= 4 {
                            let auth_type = i32::from_be_bytes([
                                payload[0], payload[1], payload[2], payload[3],
                            ]);
                            if auth_type == 0 {
                                debug!("Authentication successful");
                            }
                        }
                        self.client.write_all(&encoded).await?;
                        self.client.flush().await?;
                    }
                    BackendMessage::ReadyForQuery(_) => {
                        // Authentication complete, send to client
                        self.client.write_all(&encoded).await?;
                        self.client.flush().await?;
                        debug!("Backend ready, authentication phase complete");
                        return Ok(());
                    }
                    BackendMessage::ErrorResponse(_) => {
                        // Forward error to client
                        self.client.write_all(&encoded).await?;
                        self.client.flush().await?;
                        anyhow::bail!("Backend authentication failed");
                    }
                    _ => {
                        // Relay other messages (ParameterStatus, BackendKeyData, etc.)
                        self.client.write_all(&encoded).await?;
                        self.client.flush().await?;
                    }
                }
            }

            // Check if backend needs password from client
            // For now, we only support trust/md5/scram-sha-256 passthrough
            if !self.client_buffer.is_empty() {
                // There might be a password message waiting
                if let Some(msg) = parse_frontend_message(&mut self.client_buffer)? {
                    if let FrontendMessage::Password(payload) = msg {
                        // Forward password to backend
                        let mut pw_msg = BytesMut::new();
                        pw_msg.extend_from_slice(b"p");
                        pw_msg.extend_from_slice(&((4 + payload.len()) as i32).to_be_bytes());
                        pw_msg.extend_from_slice(&payload);
                        backend.write_all(&pw_msg).await?;
                        backend.flush().await?;
                    }
                }
            }

            // Read password from client if needed
            tokio::select! {
                result = self.client.read_buf(&mut self.client_buffer) => {
                    let n = result?;
                    if n == 0 {
                        anyhow::bail!("Client disconnected during authentication");
                    }

                    // Check for password message
                    while let Some(msg) = parse_frontend_message(&mut self.client_buffer)? {
                        if let FrontendMessage::Password(payload) = msg {
                            let mut pw_msg = BytesMut::new();
                            pw_msg.extend_from_slice(b"p");
                            pw_msg.extend_from_slice(&((4 + payload.len()) as i32).to_be_bytes());
                            pw_msg.extend_from_slice(&payload);
                            backend.write_all(&pw_msg).await?;
                            backend.flush().await?;
                        }
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {
                    // Small timeout to check backend again
                }
            }
        }
    }

    /// Main query relay loop
    async fn main_loop(&mut self) -> anyhow::Result<()> {
        loop {
            // Split borrows: we need separate access to client, backend, and buffers
            let backend = self.backend.as_mut().unwrap();

            tokio::select! {
                // Read from client
                result = self.client.read_buf(&mut self.client_buffer) => {
                    let n = result?;
                    if n == 0 {
                        debug!("Client disconnected");
                        return Ok(());
                    }
                }

                // Read from backend
                result = backend.read_buf(&mut self.backend_buffer) => {
                    let n = result?;
                    if n == 0 {
                        debug!("Backend disconnected");
                        return Ok(());
                    }
                }
            }

            // Process any client messages (outside of select to avoid borrow issues)
            while let Some(msg) = parse_frontend_message(&mut self.client_buffer)? {
                match msg {
                    FrontendMessage::Terminate => {
                        debug!("Client sent Terminate");
                        let backend = self.backend.as_mut().unwrap();
                        backend.write_all(&[b'X', 0, 0, 0, 4]).await?;
                        return Ok(());
                    }
                    FrontendMessage::Query(query) => {
                        let rewritten = self.rewrite_query(&query).await;
                        debug!(original = %query, rewritten = %rewritten, "Query");
                        let encoded = encode_query(&rewritten);
                        let backend = self.backend.as_mut().unwrap();
                        backend.write_all(&encoded).await?;
                    }
                    FrontendMessage::Parse {
                        name,
                        query,
                        param_types,
                    } => {
                        let rewritten = self.rewrite_query(&query).await;
                        debug!(original = %query, rewritten = %rewritten, "Parse");
                        let encoded = encode_parse(&name, &rewritten, &param_types);
                        let backend = self.backend.as_mut().unwrap();
                        backend.write_all(&encoded).await?;
                    }
                    FrontendMessage::Bind(payload) => {
                        forward_raw_to_backend(self.backend.as_mut().unwrap(), b'B', &payload)
                            .await?;
                    }
                    FrontendMessage::Describe(payload) => {
                        forward_raw_to_backend(self.backend.as_mut().unwrap(), b'D', &payload)
                            .await?;
                    }
                    FrontendMessage::Execute(payload) => {
                        forward_raw_to_backend(self.backend.as_mut().unwrap(), b'E', &payload)
                            .await?;
                    }
                    FrontendMessage::Sync => {
                        let backend = self.backend.as_mut().unwrap();
                        backend.write_all(&[b'S', 0, 0, 0, 4]).await?;
                    }
                    FrontendMessage::Other { tag, payload } => {
                        forward_raw_to_backend(self.backend.as_mut().unwrap(), tag, &payload)
                            .await?;
                    }
                    _ => {
                        warn!("Unexpected message in query phase: {:?}", msg);
                    }
                }
            }

            // Relay backend messages to client
            while let Some(msg) = parse_backend_message(&mut self.backend_buffer)? {
                let encoded = encode_backend_message(&msg);
                self.client.write_all(&encoded).await?;
            }
            // Flush after processing all pending backend messages
            self.client.flush().await?;
        }
    }

    /// Send error to client and optionally close connection
    #[allow(dead_code)]
    async fn send_error(&mut self, message: &str) -> anyhow::Result<()> {
        let error_msg = encode_error("ERROR", "08000", message);
        self.client.write_all(&error_msg).await?;

        // Send ReadyForQuery if we're in ready state
        if self.state == ConnectionState::Ready {
            let ready = protocol::encode_ready_for_query(b'I');
            self.client.write_all(&ready).await?;
        }

        self.client.flush().await?;
        Ok(())
    }

    /// Check if query is a session switch command (SET astapa.session)
    /// Returns Some((new_session_id, rest_of_query)) if it's a session switch, None otherwise
    fn parse_session_switch(&self, query: &str) -> Option<(Option<Uuid>, Option<String>)> {
        let trimmed = query.trim();
        let upper = trimmed.to_uppercase();

        // Handle: SET astapa.session TO 'uuid' or SET astapa.session = 'uuid'
        if upper.starts_with("SET ASTAPA.SESSION") || upper.starts_with("SET ASTAPA_SESSION") {
            // Find the value - it's quoted
            let after_set = &trimmed[19..]; // Skip "SET astapa.session" or "SET astapa_session"

            // Find the quoted value
            if let Some(quote_start) = after_set.find('\'').or_else(|| after_set.find('"')) {
                let quote_char = after_set.chars().nth(quote_start).unwrap();
                let value_start = quote_start + 1;
                if let Some(quote_end) = after_set[value_start..].find(quote_char) {
                    let value = &after_set[value_start..value_start + quote_end];

                    // Check for remaining statements after the SET
                    let rest_start = value_start + quote_end + 1;
                    let rest = after_set[rest_start..].trim();
                    let remaining = if rest.starts_with(';') {
                        let after_semi = rest[1..].trim();
                        if after_semi.is_empty() {
                            None
                        } else {
                            Some(after_semi.to_string())
                        }
                    } else {
                        None
                    };

                    if value.is_empty() {
                        return Some((None, remaining)); // Clear session
                    }
                    if let Ok(uuid) = Uuid::parse_str(value) {
                        return Some((Some(uuid), remaining));
                    }
                    return Some((None, remaining)); // Invalid UUID = clear session
                }
            }
            return Some((None, None)); // Malformed = clear session
        }

        // Handle: RESET astapa.session
        if upper.starts_with("RESET ASTAPA.SESSION") || upper.starts_with("RESET ASTAPA_SESSION") {
            let rest = trimmed[21..].trim(); // Skip "RESET astapa.session"
            let remaining = if rest.starts_with(';') {
                let after_semi = rest[1..].trim();
                if after_semi.is_empty() {
                    None
                } else {
                    Some(after_semi.to_string())
                }
            } else {
                None
            };
            return Some((None, remaining));
        }

        // Handle: DISCARD ALL (PgBouncer sends this to reset connection state)
        if upper.starts_with("DISCARD ALL") {
            return Some((None, None));
        }

        None
    }

    /// Switch to a new session (or clear session)
    fn switch_session(&mut self, new_session: Option<Uuid>) {
        match new_session {
            Some(uuid) => {
                let schema = format!("session_{}", uuid.to_string().replace('-', "_"));
                info!(session_id = %uuid, schema = %schema, "Switching to session");
                self.session_id = Some(uuid);
                self.schema_name = Some(schema.clone());
                self.rewriter = Some(Rewriter::new(&schema));
                self.ensured_tables.clear(); // Reset infrastructure cache
            }
            None => {
                info!("Clearing session (passthrough mode)");
                self.session_id = None;
                self.schema_name = None;
                self.rewriter = None;
                self.ensured_tables.clear();
            }
        }
    }

    /// Rewrite a query using the session's rewriter, ensuring infrastructure exists
    async fn rewrite_query(&mut self, query: &str) -> String {
        let trimmed = query.trim();

        // Skip empty queries
        if trimmed.is_empty() {
            return query.to_string();
        }

        // Check for session switch commands (PgBouncer compatibility)
        if let Some((new_session, remaining)) = self.parse_session_switch(trimmed) {
            self.switch_session(new_session);

            // If there are remaining statements after SET, process them
            if let Some(rest) = remaining {
                // Recursively rewrite the rest
                let rewritten_rest = Box::pin(self.rewrite_query(&rest)).await;
                return rewritten_rest;
            }

            // Return a harmless SET that backend will accept
            return "SET client_encoding TO 'UTF8'".to_string();
        }

        // Skip other SET commands
        if trimmed.to_uppercase().starts_with("SET") {
            return query.to_string();
        }

        // If we have a rewriter (session mode), use it
        if self.rewriter.is_some() {
            let schema = self.schema_name.clone().unwrap();

            // Try point lookup optimization for SELECT queries
            if let Some(direct_sql) = self.try_point_lookup_optimization(trimmed, &schema).await {
                return direct_sql;
            }

            // Check cache first
            if let Some(cached) = self.query_cache.get(&schema, trimmed) {
                QUERY_CACHE_HITS.with_label_values(&[&schema]).inc();
                // Ensure infrastructure for cached tables
                if let Err(e) = self
                    .ensure_infrastructure_for_tables(&cached.table_names, &cached.write_tables)
                    .await
                {
                    warn!(error = %e, "Failed to ensure infrastructure, query may fail");
                }
                debug!(query_type = ?cached.query_type, "Query (cached)");

                // Track writes for dirty key tracking
                self.track_dirty_keys_from_query(trimmed, &cached.query_type, &cached.write_tables);

                return cached.sql;
            }

            // Parse and rewrite
            QUERY_CACHE_MISSES.with_label_values(&[&schema]).inc();
            let rewriter = self.rewriter.as_ref().unwrap();
            match rewriter.analyze(query) {
                Ok(analysis) => {
                    // Ensure infrastructure exists for all referenced tables
                    if let Err(e) = self.ensure_infrastructure(&analysis).await {
                        warn!(error = %e, "Failed to ensure infrastructure, query may fail");
                    }
                    debug!(query_type = ?analysis.query_type, tables = ?analysis.tables, "Query analyzed");

                    // Cache the result
                    self.query_cache.insert(&schema, trimmed, &analysis);

                    // Track writes for dirty key tracking
                    let write_tables: Vec<String> = analysis
                        .tables
                        .iter()
                        .filter(|t| t.is_write_target)
                        .map(|t| t.name.clone())
                        .collect();
                    self.track_dirty_keys_from_query(trimmed, &analysis.query_type, &write_tables);

                    analysis.sql
                }
                Err(e) => {
                    // Log error but fall back to passthrough for now
                    warn!(error = %e, "Query rewrite failed, passing through");
                    query.to_string()
                }
            }
        } else {
            // No session - pass through unchanged
            query.to_string()
        }
    }

    /// Try to optimize a SELECT query by routing clean point lookups directly to base table.
    /// Returns Some(rewritten_sql) if optimization applies, None otherwise.
    async fn try_point_lookup_optimization(
        &mut self,
        query: &str,
        _schema: &str,
    ) -> Option<String> {
        // Parse the query
        let dialect = PostgreSqlDialect {};
        let statements = Parser::parse_sql(&dialect, query).ok()?;

        if statements.len() != 1 {
            return None;
        }

        // Try to detect a point lookup (returns None if table PK not in cache)
        let point_lookup = detect_point_lookup(&statements[0], &self.pk_cache)?;

        // Check if ALL requested keys are clean
        let all_clean = point_lookup
            .pk_values
            .iter()
            .all(|pk| self.dirty_tracker.is_clean(&point_lookup.table_name, pk));

        if !all_clean {
            debug!(
                table = %point_lookup.table_name,
                keys = ?point_lookup.pk_values,
                "Point lookup has dirty keys, using view"
            );
            return None;
        }

        // All keys are clean - rewrite to use public.{table} directly
        debug!(
            table = %point_lookup.table_name,
            keys = ?point_lookup.pk_values,
            "Point lookup optimization: routing to base table"
        );

        // Rewrite the query to use public.{table} instead of the view
        // Simple string replacement for the table name
        let rewritten = self.rewrite_for_direct_read(query, &point_lookup.table_name);

        Some(rewritten)
    }

    /// Rewrite a query to read directly from the public schema (for clean point lookups).
    fn rewrite_for_direct_read(&self, query: &str, table_name: &str) -> String {
        // For simple cases, we can do string replacement
        // The table might appear as: users, "users", public.users, "public"."users"
        // We want to ensure it becomes public.{table}

        // Parse and rewrite properly using sqlparser
        let dialect = PostgreSqlDialect {};
        if let Ok(mut statements) = Parser::parse_sql(&dialect, query) {
            if statements.len() == 1 {
                // Rewrite table references to public.{table}
                self.rewrite_statement_for_direct_read(&mut statements[0], table_name);
                return statements[0].to_string();
            }
        }

        // Fallback: simple string approach (less robust but handles most cases)
        query.to_string()
    }

    /// Rewrite a statement to use public.{table} for direct reads.
    fn rewrite_statement_for_direct_read(
        &self,
        stmt: &mut sqlparser::ast::Statement,
        table_name: &str,
    ) {
        use sqlparser::ast::*;

        if let Statement::Query(ref mut query) = stmt {
            if let SetExpr::Select(ref mut select) = *query.body {
                for table_with_joins in &mut select.from {
                    self.rewrite_table_factor_for_direct_read(
                        &mut table_with_joins.relation,
                        table_name,
                    );
                }
            }
        }
    }

    /// Rewrite a table factor to use public schema.
    fn rewrite_table_factor_for_direct_read(
        &self,
        table_factor: &mut sqlparser::ast::TableFactor,
        target_table: &str,
    ) {
        use sqlparser::ast::*;

        if let TableFactor::Table { ref mut name, .. } = table_factor {
            // Check if this is the target table
            if let Some(last) = name.0.last() {
                if last.value.eq_ignore_ascii_case(target_table) {
                    // Rewrite to public.{table}
                    *name = ObjectName(vec![Ident::new("public"), Ident::new(target_table)]);
                }
            }
        }
    }

    /// Track dirty keys when a write query is processed.
    fn track_dirty_keys_from_query(
        &mut self,
        query: &str,
        query_type: &QueryType,
        write_tables: &[String],
    ) {
        match query_type {
            QueryType::Insert | QueryType::Update | QueryType::Delete => {
                // For now, mark the entire table as dirty for range writes
                // A more sophisticated implementation would extract PKs from WHERE clauses
                for table in write_tables {
                    self.mark_table_dirty_from_write(query, table, query_type);
                }
            }
            _ => {}
        }
    }

    /// Mark a table dirty based on a write query.
    /// For point writes (WHERE pk = X), marks specific keys.
    /// For range writes, marks the entire table as dirty.
    fn mark_table_dirty_from_write(&mut self, query: &str, table: &str, query_type: &QueryType) {
        let table_lower = table.to_lowercase();

        // Try to extract specific PK values from WHERE clause
        if let Some(pk_column) = self.pk_cache.get(&table_lower) {
            if let Some(pk_values) = self.extract_pk_from_write_query(query, pk_column, query_type)
            {
                // Point write - mark specific keys
                for pk in pk_values {
                    debug!(table = %table, pk = %pk, "Marking key dirty");
                    self.dirty_tracker.mark_dirty(&table_lower, &pk);
                }
                return;
            }
        }

        // Range write or couldn't extract PKs - mark entire table dirty
        debug!(table = %table, "Marking entire table dirty (range write)");
        self.dirty_tracker.mark_table_fully_dirty(&table_lower);
    }

    /// Try to extract PK values from a write query's WHERE clause.
    fn extract_pk_from_write_query(
        &self,
        query: &str,
        pk_column: &str,
        query_type: &QueryType,
    ) -> Option<Vec<String>> {
        let dialect = PostgreSqlDialect {};
        let statements = Parser::parse_sql(&dialect, query).ok()?;

        if statements.len() != 1 {
            return None;
        }

        match (&statements[0], query_type) {
            (sqlparser::ast::Statement::Insert { source, .. }, QueryType::Insert) => {
                // For INSERT, try to extract PK from VALUES clause
                self.extract_pk_from_insert(source.as_ref()?, pk_column)
            }
            (sqlparser::ast::Statement::Update { selection, .. }, QueryType::Update) => {
                // For UPDATE, extract PK from WHERE clause
                selection
                    .as_ref()
                    .and_then(|sel| self.extract_pk_from_where(sel, pk_column))
            }
            (sqlparser::ast::Statement::Delete { selection, .. }, QueryType::Delete) => {
                // For DELETE, extract PK from WHERE clause
                selection
                    .as_ref()
                    .and_then(|sel| self.extract_pk_from_where(sel, pk_column))
            }
            _ => None,
        }
    }

    /// Extract PK values from an INSERT's VALUES clause.
    fn extract_pk_from_insert(
        &self,
        source: &sqlparser::ast::Query,
        _pk_column: &str,
    ) -> Option<Vec<String>> {
        // INSERT ... SELECT is a range write
        if let sqlparser::ast::SetExpr::Values(ref values) = *source.body {
            // For now, mark as range write - extracting PKs from VALUES requires
            // knowing column positions which adds complexity
            // TODO: Implement proper PK extraction from VALUES
            let _ = values;
        }
        None
    }

    /// Extract PK values from a WHERE clause (for UPDATE/DELETE).
    fn extract_pk_from_where(
        &self,
        expr: &sqlparser::ast::Expr,
        pk_column: &str,
    ) -> Option<Vec<String>> {
        use sqlparser::ast::*;

        match expr {
            Expr::BinaryOp { left, op, right } => {
                match op {
                    BinaryOperator::Eq => {
                        // Check if this is pk = value
                        if self.is_column_ref(left, pk_column) {
                            return self.extract_literal(right).map(|v| vec![v]);
                        }
                        if self.is_column_ref(right, pk_column) {
                            return self.extract_literal(left).map(|v| vec![v]);
                        }
                        None
                    }
                    BinaryOperator::And => {
                        // Try both sides
                        self.extract_pk_from_where(left, pk_column)
                            .or_else(|| self.extract_pk_from_where(right, pk_column))
                    }
                    _ => None, // Range or OR conditions
                }
            }
            Expr::InList {
                expr,
                list,
                negated,
            } if !negated => {
                if self.is_column_ref(expr, pk_column) {
                    let values: Vec<String> = list
                        .iter()
                        .filter_map(|e| self.extract_literal(e))
                        .collect();
                    if values.len() == list.len() {
                        return Some(values);
                    }
                }
                None
            }
            Expr::Nested(inner) => self.extract_pk_from_where(inner, pk_column),
            _ => None,
        }
    }

    /// Check if an expression is a column reference.
    fn is_column_ref(&self, expr: &sqlparser::ast::Expr, column_name: &str) -> bool {
        use sqlparser::ast::*;
        match expr {
            Expr::Identifier(ident) => ident.value.eq_ignore_ascii_case(column_name),
            Expr::CompoundIdentifier(idents) => idents
                .last()
                .map(|i| i.value.eq_ignore_ascii_case(column_name))
                .unwrap_or(false),
            _ => false,
        }
    }

    /// Extract a literal value from an expression.
    fn extract_literal(&self, expr: &sqlparser::ast::Expr) -> Option<String> {
        use sqlparser::ast::*;
        match expr {
            Expr::Value(v) => match v {
                Value::Number(n, _) => Some(n.clone()),
                Value::SingleQuotedString(s) => Some(s.clone()),
                Value::DoubleQuotedString(s) => Some(s.clone()),
                _ => None,
            },
            Expr::UnaryOp {
                op: UnaryOperator::Minus,
                expr,
            } => {
                if let Expr::Value(Value::Number(n, _)) = expr.as_ref() {
                    Some(format!("-{}", n))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Ensure infrastructure for a list of table names (used with cached queries)
    async fn ensure_infrastructure_for_tables(
        &mut self,
        table_names: &[String],
        _write_tables: &[String],
    ) -> anyhow::Result<()> {
        let schema_name = match &self.schema_name {
            Some(s) => s.clone(),
            None => return Ok(()),
        };

        // Collect tables that need infrastructure
        let mut needs_view: Vec<String> = Vec::new();

        for table_name in table_names {
            let infra = self
                .ensured_tables
                .get(table_name)
                .cloned()
                .unwrap_or_default();
            if !infra.view {
                needs_view.push(table_name.clone());
            }
        }

        // Create infrastructure in parallel for better performance
        if !needs_view.is_empty() {
            let futures: Vec<_> = needs_view
                .iter()
                .map(|table_name| {
                    let schema = schema_name.clone();
                    let table = table_name.clone();
                    let schema_manager = self.schema_manager.clone();
                    let pool = self.pool.clone();
                    let session_id = self.session_id;
                    async move {
                        Self::ensure_view_static(
                            &schema_manager,
                            &pool,
                            &schema,
                            &table,
                            session_id,
                        )
                        .await
                        .map(|pk| (table, pk))
                    }
                })
                .collect();

            let results = futures::future::join_all(futures).await;
            for result in results {
                match result {
                    Ok((table_name, pk)) => {
                        let infra = self.ensured_tables.entry(table_name.clone()).or_default();
                        infra.view = true;
                        infra.shadow = true;
                        infra.deleted = true;
                        // Cache the PK for point lookup optimization
                        self.pk_cache.insert(table_name.to_lowercase(), pk);
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to create infrastructure");
                    }
                }
            }
        }

        Ok(())
    }

    /// Ensure required infrastructure (views, shadow tables) exists for a query
    async fn ensure_infrastructure(&mut self, analysis: &QueryAnalysis) -> anyhow::Result<()> {
        let schema_name = match &self.schema_name {
            Some(s) => s.clone(),
            None => return Ok(()), // No session, nothing to ensure
        };

        // First pass: collect what needs to be done
        let mut needs_view: Vec<String> = Vec::new();
        let needs_shadow: Vec<String> = Vec::new();
        let mut needs_deleted: Vec<String> = Vec::new();

        for table_ref in &analysis.tables {
            let table_name = &table_ref.name;
            let infra = self
                .ensured_tables
                .get(table_name)
                .cloned()
                .unwrap_or_default();

            match analysis.query_type {
                QueryType::Select if !infra.view => {
                    needs_view.push(table_name.clone());
                }
                QueryType::Insert | QueryType::Update => {
                    // All operations go through views now (INSTEAD OF triggers handle writes)
                    if table_ref.is_write_target && !infra.view {
                        needs_view.push(table_name.clone());
                    }
                    if !table_ref.is_write_target && !infra.view {
                        needs_view.push(table_name.clone());
                    }
                }
                QueryType::Delete => {
                    // All operations go through views now
                    if table_ref.is_write_target && !infra.view {
                        needs_view.push(table_name.clone());
                    }
                    if table_ref.is_write_target && !infra.deleted {
                        needs_deleted.push(table_name.clone());
                    }
                    if !table_ref.is_write_target && !infra.view {
                        needs_view.push(table_name.clone());
                    }
                }
                _ => {}
            }
        }

        // Second pass: create infrastructure
        // Note: queries in a single connection are sequential, so no in-connection race.
        // Cross-connection races are handled by advisory locks in SchemaManager.
        for table_name in needs_view {
            self.ensure_view(&schema_name, &table_name).await?;
            // Mark after success - ensure_view creates shadow + deleted + view
            let infra = self.ensured_tables.entry(table_name.clone()).or_default();
            infra.view = true;
            infra.shadow = true;
            infra.deleted = true;
            // Fetch and cache the PK for point lookup optimization
            if let Ok(pk) = self
                .schema_manager
                .get_primary_key("public", &table_name)
                .await
            {
                self.pk_cache.insert(table_name.to_lowercase(), pk);
            }
        }

        for table_name in needs_shadow {
            self.ensure_shadow(&schema_name, &table_name).await?;
            let infra = self.ensured_tables.entry(table_name.clone()).or_default();
            infra.shadow = true;
        }

        for table_name in needs_deleted {
            self.ensure_deleted(&schema_name, &table_name).await?;
            let infra = self.ensured_tables.entry(table_name.clone()).or_default();
            infra.deleted = true;
        }

        Ok(())
    }

    /// Ensure shadow table exists for a table
    async fn ensure_shadow(&self, schema_name: &str, table_name: &str) -> anyhow::Result<()> {
        debug!(
            schema = schema_name,
            table = table_name,
            "Ensuring shadow table"
        );
        self.schema_manager
            .create_shadow_table(schema_name, "public", table_name)
            .await?;
        Ok(())
    }

    /// Ensure deleted tracking table exists for a table
    async fn ensure_deleted(&self, schema_name: &str, table_name: &str) -> anyhow::Result<()> {
        debug!(
            schema = schema_name,
            table = table_name,
            "Ensuring deleted table"
        );
        self.schema_manager
            .create_deleted_table(schema_name, "public", table_name)
            .await?;
        Ok(())
    }

    /// Ensure view exists for a table (creates shadow + deleted + view) - static version for parallel calls
    async fn ensure_view_static(
        schema_manager: &SchemaManager,
        pool: &Arc<Pool>,
        schema_name: &str,
        table_name: &str,
        session_id: Option<Uuid>,
    ) -> anyhow::Result<String> {
        debug!(
            schema = schema_name,
            table = table_name,
            "Ensuring view (parallel)"
        );

        // Create shadow table first
        schema_manager
            .create_shadow_table(schema_name, "public", table_name)
            .await?;

        // Create deleted table
        schema_manager
            .create_deleted_table(schema_name, "public", table_name)
            .await?;

        // Get primary key for view creation
        let pk = schema_manager.get_primary_key("public", table_name).await?;

        // Create union view
        schema_manager
            .create_union_view(schema_name, "public", table_name, &pk)
            .await?;

        // Update session's tables state in database so approval can find it
        if let Some(session_id) = session_id {
            let client = pool.get().await?;
            let table_state = serde_json::json!({
                "shadow_created": true,
                "view_created": true,
                "primary_key": pk
            });

            client
                .execute(
                    r#"
                    UPDATE _astapa_sessions
                    SET tables = jsonb_set(
                        COALESCE(tables, '{}'::jsonb),
                        $2::text[],
                        $3::jsonb
                    )
                    WHERE id = $1
                    "#,
                    &[&session_id, &vec![table_name.to_string()], &table_state],
                )
                .await?;

            debug!(
                session_id = %session_id,
                table = table_name,
                "Updated session table state in database"
            );
        }

        Ok(pk)
    }

    /// Ensure view exists for a table (creates shadow + deleted + view)
    async fn ensure_view(&self, schema_name: &str, table_name: &str) -> anyhow::Result<()> {
        debug!(schema = schema_name, table = table_name, "Ensuring view");

        // Create shadow table first
        self.schema_manager
            .create_shadow_table(schema_name, "public", table_name)
            .await?;

        // Create deleted table
        self.schema_manager
            .create_deleted_table(schema_name, "public", table_name)
            .await?;

        // Get primary key for view creation
        let pk = self
            .schema_manager
            .get_primary_key("public", table_name)
            .await?;

        // Create union view
        self.schema_manager
            .create_union_view(schema_name, "public", table_name, &pk)
            .await?;

        // Update session's tables state in database so approval can find it
        if let Some(session_id) = self.session_id {
            self.update_session_table_state(session_id, table_name, &pk)
                .await?;
        }

        Ok(())
    }

    /// Update the session's tables state in the database
    async fn update_session_table_state(
        &self,
        session_id: Uuid,
        table_name: &str,
        primary_key: &str,
    ) -> anyhow::Result<()> {
        let client = self.pool.get().await?;

        // Use jsonb_set to add/update the table entry in the tables column
        let table_state = serde_json::json!({
            "shadow_created": true,
            "view_created": true,
            "primary_key": primary_key
        });

        client
            .execute(
                r#"
                UPDATE _astapa_sessions
                SET tables = jsonb_set(
                    COALESCE(tables, '{}'::jsonb),
                    $2::text[],
                    $3::jsonb
                )
                WHERE id = $1
                "#,
                &[&session_id, &vec![table_name.to_string()], &table_state],
            )
            .await?;

        debug!(
            session_id = %session_id,
            table = table_name,
            "Updated session table state in database"
        );

        Ok(())
    }
}

/// Forward a raw message to backend (free function to avoid borrow issues)
async fn forward_raw_to_backend(
    backend: &mut TcpStream,
    tag: u8,
    payload: &[u8],
) -> anyhow::Result<()> {
    use bytes::BufMut;
    let mut msg = BytesMut::new();
    msg.put_u8(tag);
    msg.put_i32(4 + payload.len() as i32);
    msg.put_slice(payload);
    backend.write_all(&msg).await?;
    Ok(())
}

/// Check if error is a normal connection close
fn is_connection_closed_error(e: &anyhow::Error) -> bool {
    if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
        matches!(
            io_err.kind(),
            std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::UnexpectedEof
        )
    } else {
        e.to_string().contains("disconnected")
    }
}

/// Extract a value from Postgres options string (e.g., "-c key=value -c other=x")
/// Uses char_indices to ensure safe UTF-8 boundary handling.
fn extract_option_value(options: &str, key: &str) -> Option<String> {
    let pattern = format!("-c {}=", key);
    if let Some(pos) = options.find(&pattern) {
        let start = pos + pattern.len();
        let rest = &options[start..];
        // Use char_indices to find whitespace safely across UTF-8 boundaries
        let end = rest
            .char_indices()
            .find(|(_, c)| c.is_whitespace())
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        Some(rest[..end].to_string())
    } else {
        None
    }
}

/// Strip a key=value pair from Postgres options string
/// Uses char_indices to ensure safe UTF-8 boundary handling.
fn strip_option_value(options: &str, key: &str) -> String {
    let pattern = format!("-c {}=", key);
    if let Some(pos) = options.find(&pattern) {
        let before = &options[..pos];
        let rest = &options[pos + pattern.len()..];
        // Use char_indices to find whitespace safely across UTF-8 boundaries
        let end = rest
            .char_indices()
            .find(|(_, c)| c.is_whitespace())
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        let after = &rest[end..];
        format!("{}{}", before.trim(), after).trim().to_string()
    } else {
        options.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_option_value() {
        let opts = "-c astapa_session=abc-123 -c other=xyz";
        assert_eq!(
            extract_option_value(opts, "astapa_session"),
            Some("abc-123".to_string())
        );
        assert_eq!(extract_option_value(opts, "other"), Some("xyz".to_string()));
        assert_eq!(extract_option_value(opts, "missing"), None);
    }

    #[test]
    fn test_strip_option_value() {
        let opts = "-c astapa_session=abc-123 -c other=xyz";
        assert_eq!(strip_option_value(opts, "astapa_session"), "-c other=xyz");

        let opts2 = "-c astapa_session=abc-123";
        assert_eq!(strip_option_value(opts2, "astapa_session"), "");
    }

    #[test]
    fn test_rewriter_integration() {
        // Test that the Rewriter properly rewrites queries
        let rewriter = Rewriter::new("session_abc123");

        // SELECT should use view
        let (sql, qt) = rewriter.rewrite("SELECT * FROM users").unwrap();
        assert_eq!(qt, QueryType::Select);
        assert!(sql.contains("session_abc123._view_users"));

        // INSERT should use shadow table
        let (sql, qt) = rewriter
            .rewrite("INSERT INTO users (name) VALUES ('test')")
            .unwrap();
        assert_eq!(qt, QueryType::Insert);
        assert!(sql.contains("session_abc123._view_users"));
    }
}
