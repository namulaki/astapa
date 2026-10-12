#![allow(dead_code)]
//! Postgres wire protocol message parsing and encoding helpers.
//!
//! Reference: https://www.postgresql.org/docs/current/protocol-message-formats.html

use bytes::{Buf, BufMut, BytesMut};
use std::collections::HashMap;
use thiserror::Error;

/// SSL request magic number (1234 << 16 | 5679)
pub const SSL_REQUEST_CODE: i32 = 80877103;

/// Protocol version 3.0 (3 << 16 | 0)
pub const PROTOCOL_VERSION_3: i32 = 196608;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("Incomplete message, need more data")]
    Incomplete,
    #[error("Invalid message format: {0}")]
    InvalidFormat(String),
    #[error("Unsupported protocol version: {0}")]
    UnsupportedVersion(i32),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Message types from frontend (client)
#[derive(Debug, Clone, PartialEq)]
pub enum FrontendMessage {
    /// SSL negotiation request (no type byte)
    SslRequest,
    /// Startup message with parameters (no type byte)
    Startup(StartupMessage),
    /// Simple query ('Q')
    Query(String),
    /// Parse message for extended query ('P')
    Parse {
        name: String,
        query: String,
        param_types: Vec<i32>,
    },
    /// Bind message ('B')
    Bind(Vec<u8>),
    /// Describe message ('D')
    Describe(Vec<u8>),
    /// Execute message ('E')
    Execute(Vec<u8>),
    /// Sync message ('S')
    Sync,
    /// Terminate message ('X')
    Terminate,
    /// Password message ('p')
    Password(Vec<u8>),
    /// Other messages we pass through
    Other { tag: u8, payload: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct StartupMessage {
    pub protocol_version: i32,
    pub parameters: HashMap<String, String>,
}

/// Message types from backend (server)
#[derive(Debug, Clone)]
pub enum BackendMessage {
    /// Authentication request ('R')
    Authentication(Vec<u8>),
    /// Parameter status ('S')
    ParameterStatus(Vec<u8>),
    /// Backend key data ('K')
    BackendKeyData(Vec<u8>),
    /// Ready for query ('Z')
    ReadyForQuery(u8), // transaction status
    /// Row description ('T')
    RowDescription(Vec<u8>),
    /// Data row ('D')
    DataRow(Vec<u8>),
    /// Command complete ('C')
    CommandComplete(Vec<u8>),
    /// Error response ('E')
    ErrorResponse(Vec<u8>),
    /// Notice response ('N')
    NoticeResponse(Vec<u8>),
    /// Parse complete ('1')
    ParseComplete,
    /// Bind complete ('2')
    BindComplete,
    /// Close complete ('3')
    CloseComplete,
    /// Empty query response ('I')
    EmptyQueryResponse,
    /// No data ('n')
    NoData,
    /// Other messages
    Other { tag: u8, payload: Vec<u8> },
}

/// Try to parse a frontend message from buffer.
/// Returns Ok(Some(msg)) if complete message parsed.
/// Returns Ok(None) if more data needed.
/// Returns Err on invalid data.
pub fn parse_frontend_message(
    buf: &mut BytesMut,
) -> Result<Option<FrontendMessage>, ProtocolError> {
    if buf.len() < 4 {
        return Ok(None);
    }

    // Peek at the first byte to determine message type
    let _first_byte = buf[0];

    // Check for startup-phase messages (no type byte)
    if is_startup_message(buf)? {
        return parse_startup_phase_message(buf);
    }

    // Regular message: type byte + int32 length + payload
    if buf.len() < 5 {
        return Ok(None);
    }

    let len = i32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    if len < 4 {
        return Err(ProtocolError::InvalidFormat(
            "Message length too small".into(),
        ));
    }

    let total_len = 1 + len; // type byte + length field value (includes length itself)
    if buf.len() < total_len {
        return Ok(None);
    }

    let tag = buf[0];
    buf.advance(5); // skip type byte and length
    let payload_len = len - 4;
    let payload = buf.split_to(payload_len);

    let msg = match tag {
        b'Q' => {
            // Query: string query\0
            let query = parse_cstring(&payload)?;
            FrontendMessage::Query(query)
        }
        b'P' => {
            // Parse: string name\0 | string query\0 | int16 param_count | oid[param_count]
            let mut cursor = &payload[..];
            let name = read_cstring(&mut cursor)?;
            let query = read_cstring(&mut cursor)?;
            let param_count = if cursor.len() >= 2 {
                cursor.get_i16() as usize
            } else {
                0
            };
            let mut param_types = Vec::with_capacity(param_count);
            for _ in 0..param_count {
                if cursor.len() >= 4 {
                    param_types.push(cursor.get_i32());
                }
            }
            FrontendMessage::Parse {
                name,
                query,
                param_types,
            }
        }
        b'B' => FrontendMessage::Bind(payload.to_vec()),
        b'D' => FrontendMessage::Describe(payload.to_vec()),
        b'E' => FrontendMessage::Execute(payload.to_vec()),
        b'S' => FrontendMessage::Sync,
        b'X' => FrontendMessage::Terminate,
        b'p' => FrontendMessage::Password(payload.to_vec()),
        _ => FrontendMessage::Other {
            tag,
            payload: payload.to_vec(),
        },
    };

    Ok(Some(msg))
}

/// Check if buffer contains a startup-phase message (SSL request or startup message)
fn is_startup_message(buf: &BytesMut) -> Result<bool, ProtocolError> {
    if buf.len() < 8 {
        return Ok(false);
    }

    let len = i32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;

    // SSL request is exactly 8 bytes
    if len == 8 {
        let code = i32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
        if code == SSL_REQUEST_CODE {
            return Ok(true);
        }
    }

    // Startup message starts with length, then protocol version
    if (8..=10000).contains(&len) {
        let version = i32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
        if version == PROTOCOL_VERSION_3 {
            return Ok(true);
        }
    }

    Ok(false)
}

fn parse_startup_phase_message(
    buf: &mut BytesMut,
) -> Result<Option<FrontendMessage>, ProtocolError> {
    if buf.len() < 8 {
        return Ok(None);
    }

    let len = i32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;

    if buf.len() < len {
        return Ok(None);
    }

    let code = i32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);

    if code == SSL_REQUEST_CODE {
        buf.advance(8);
        return Ok(Some(FrontendMessage::SslRequest));
    }

    if code == PROTOCOL_VERSION_3 {
        buf.advance(8);
        let params_len = len - 8;
        let params_data = buf.split_to(params_len);

        let mut parameters = HashMap::new();
        let mut cursor = &params_data[..];

        while !cursor.is_empty() && cursor[0] != 0 {
            let key = read_cstring(&mut cursor)?;
            if cursor.is_empty() {
                break;
            }
            let value = read_cstring(&mut cursor)?;
            parameters.insert(key, value);
        }

        return Ok(Some(FrontendMessage::Startup(StartupMessage {
            protocol_version: code,
            parameters,
        })));
    }

    Err(ProtocolError::UnsupportedVersion(code))
}

/// Parse a backend message from buffer
pub fn parse_backend_message(buf: &mut BytesMut) -> Result<Option<BackendMessage>, ProtocolError> {
    if buf.len() < 5 {
        return Ok(None);
    }

    let tag = buf[0];
    let len = i32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;

    if len < 4 {
        return Err(ProtocolError::InvalidFormat(
            "Message length too small".into(),
        ));
    }

    let total_len = 1 + len;
    if buf.len() < total_len {
        return Ok(None);
    }

    buf.advance(5);
    let payload_len = len - 4;
    let payload = buf.split_to(payload_len);

    let msg = match tag {
        b'R' => BackendMessage::Authentication(payload.to_vec()),
        b'S' => BackendMessage::ParameterStatus(payload.to_vec()),
        b'K' => BackendMessage::BackendKeyData(payload.to_vec()),
        b'Z' => {
            let status = if payload.is_empty() { b'I' } else { payload[0] };
            BackendMessage::ReadyForQuery(status)
        }
        b'T' => BackendMessage::RowDescription(payload.to_vec()),
        b'D' => BackendMessage::DataRow(payload.to_vec()),
        b'C' => BackendMessage::CommandComplete(payload.to_vec()),
        b'E' => BackendMessage::ErrorResponse(payload.to_vec()),
        b'N' => BackendMessage::NoticeResponse(payload.to_vec()),
        b'1' => BackendMessage::ParseComplete,
        b'2' => BackendMessage::BindComplete,
        b'3' => BackendMessage::CloseComplete,
        b'I' => BackendMessage::EmptyQueryResponse,
        b'n' => BackendMessage::NoData,
        _ => BackendMessage::Other {
            tag,
            payload: payload.to_vec(),
        },
    };

    Ok(Some(msg))
}

// ============================================================================
// Encoding helpers
// ============================================================================

/// Encode a simple Query message
pub fn encode_query(query: &str) -> BytesMut {
    let mut buf = BytesMut::new();
    buf.put_u8(b'Q');
    let len = 4 + query.len() + 1; // length field + query + null terminator
    buf.put_i32(len as i32);
    buf.put_slice(query.as_bytes());
    buf.put_u8(0);
    buf
}

/// Encode a Parse message
pub fn encode_parse(name: &str, query: &str, param_types: &[i32]) -> BytesMut {
    let mut buf = BytesMut::new();
    buf.put_u8(b'P');

    let len = 4 + name.len() + 1 + query.len() + 1 + 2 + (param_types.len() * 4);
    buf.put_i32(len as i32);
    buf.put_slice(name.as_bytes());
    buf.put_u8(0);
    buf.put_slice(query.as_bytes());
    buf.put_u8(0);
    buf.put_i16(param_types.len() as i16);
    for oid in param_types {
        buf.put_i32(*oid);
    }
    buf
}

/// Encode a startup message
/// Note: Parameters are encoded in a specific order (user, database first) for compatibility
/// with some proxies that may be sensitive to parameter ordering.
pub fn encode_startup(parameters: &HashMap<String, String>) -> BytesMut {
    let mut params_buf = BytesMut::new();

    // Encode in a fixed order: user first, then database, then others alphabetically
    // This ensures consistent ordering regardless of HashMap iteration
    let priority_keys = ["user", "database"];

    for key in &priority_keys {
        if let Some(value) = parameters.get(*key) {
            params_buf.put_slice(key.as_bytes());
            params_buf.put_u8(0);
            params_buf.put_slice(value.as_bytes());
            params_buf.put_u8(0);
        }
    }

    // Then encode remaining parameters (sorted for consistency)
    let mut other_keys: Vec<_> = parameters
        .keys()
        .filter(|k| !priority_keys.contains(&k.as_str()))
        .collect();
    other_keys.sort();

    for key in other_keys {
        if let Some(value) = parameters.get(key) {
            params_buf.put_slice(key.as_bytes());
            params_buf.put_u8(0);
            params_buf.put_slice(value.as_bytes());
            params_buf.put_u8(0);
        }
    }

    params_buf.put_u8(0); // terminator

    let mut buf = BytesMut::new();
    let len = 4 + 4 + params_buf.len(); // length + version + params
    buf.put_i32(len as i32);
    buf.put_i32(PROTOCOL_VERSION_3);
    buf.put_slice(&params_buf);
    buf
}

/// Encode an error response
pub fn encode_error(severity: &str, code: &str, message: &str) -> BytesMut {
    let mut buf = BytesMut::new();
    buf.put_u8(b'E');

    let mut fields = BytesMut::new();
    // Severity
    fields.put_u8(b'S');
    fields.put_slice(severity.as_bytes());
    fields.put_u8(0);
    // Code
    fields.put_u8(b'C');
    fields.put_slice(code.as_bytes());
    fields.put_u8(0);
    // Message
    fields.put_u8(b'M');
    fields.put_slice(message.as_bytes());
    fields.put_u8(0);
    // Terminator
    fields.put_u8(0);

    buf.put_i32(4 + fields.len() as i32);
    buf.put_slice(&fields);
    buf
}

/// Encode ReadyForQuery message
pub fn encode_ready_for_query(status: u8) -> BytesMut {
    let mut buf = BytesMut::new();
    buf.put_u8(b'Z');
    buf.put_i32(5); // length = 4 + 1
    buf.put_u8(status);
    buf
}

/// Encode raw backend message for passthrough
pub fn encode_backend_message(msg: &BackendMessage) -> BytesMut {
    let mut buf = BytesMut::new();
    match msg {
        BackendMessage::Authentication(payload) => encode_raw_message(&mut buf, b'R', payload),
        BackendMessage::ParameterStatus(payload) => encode_raw_message(&mut buf, b'S', payload),
        BackendMessage::BackendKeyData(payload) => encode_raw_message(&mut buf, b'K', payload),
        BackendMessage::ReadyForQuery(status) => {
            buf.put_u8(b'Z');
            buf.put_i32(5);
            buf.put_u8(*status);
        }
        BackendMessage::RowDescription(payload) => encode_raw_message(&mut buf, b'T', payload),
        BackendMessage::DataRow(payload) => encode_raw_message(&mut buf, b'D', payload),
        BackendMessage::CommandComplete(payload) => encode_raw_message(&mut buf, b'C', payload),
        BackendMessage::ErrorResponse(payload) => encode_raw_message(&mut buf, b'E', payload),
        BackendMessage::NoticeResponse(payload) => encode_raw_message(&mut buf, b'N', payload),
        BackendMessage::ParseComplete => {
            buf.put_u8(b'1');
            buf.put_i32(4);
        }
        BackendMessage::BindComplete => {
            buf.put_u8(b'2');
            buf.put_i32(4);
        }
        BackendMessage::CloseComplete => {
            buf.put_u8(b'3');
            buf.put_i32(4);
        }
        BackendMessage::EmptyQueryResponse => {
            buf.put_u8(b'I');
            buf.put_i32(4);
        }
        BackendMessage::NoData => {
            buf.put_u8(b'n');
            buf.put_i32(4);
        }
        BackendMessage::Other { tag, payload } => encode_raw_message(&mut buf, *tag, payload),
    }
    buf
}

fn encode_raw_message(buf: &mut BytesMut, tag: u8, payload: &[u8]) {
    buf.put_u8(tag);
    buf.put_i32(4 + payload.len() as i32);
    buf.put_slice(payload);
}

// ============================================================================
// String helpers
// ============================================================================

fn parse_cstring(data: &[u8]) -> Result<String, ProtocolError> {
    let null_pos = data
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| ProtocolError::InvalidFormat("Missing null terminator".into()))?;
    String::from_utf8(data[..null_pos].to_vec())
        .map_err(|e| ProtocolError::InvalidFormat(format!("Invalid UTF-8: {}", e)))
}

fn read_cstring(cursor: &mut &[u8]) -> Result<String, ProtocolError> {
    let null_pos = cursor
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| ProtocolError::InvalidFormat("Missing null terminator".into()))?;
    let s = String::from_utf8(cursor[..null_pos].to_vec())
        .map_err(|e| ProtocolError::InvalidFormat(format!("Invalid UTF-8: {}", e)))?;
    *cursor = &cursor[null_pos + 1..];
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ssl_request() {
        let mut buf = BytesMut::new();
        buf.put_i32(8);
        buf.put_i32(SSL_REQUEST_CODE);

        let msg = parse_frontend_message(&mut buf).unwrap().unwrap();
        assert_eq!(msg, FrontendMessage::SslRequest);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_parse_startup_message() {
        let mut buf = BytesMut::new();
        let params = vec![("user", "postgres"), ("database", "test")];

        let mut params_len = 0;
        for (k, v) in &params {
            params_len += k.len() + 1 + v.len() + 1;
        }
        params_len += 1; // terminator

        buf.put_i32(8 + params_len as i32);
        buf.put_i32(PROTOCOL_VERSION_3);
        for (k, v) in &params {
            buf.put_slice(k.as_bytes());
            buf.put_u8(0);
            buf.put_slice(v.as_bytes());
            buf.put_u8(0);
        }
        buf.put_u8(0);

        let msg = parse_frontend_message(&mut buf).unwrap().unwrap();
        if let FrontendMessage::Startup(startup) = msg {
            assert_eq!(
                startup.parameters.get("user"),
                Some(&"postgres".to_string())
            );
            assert_eq!(
                startup.parameters.get("database"),
                Some(&"test".to_string())
            );
        } else {
            panic!("Expected Startup message");
        }
    }

    #[test]
    fn test_parse_query() {
        let query = "SELECT 1";
        let mut buf = BytesMut::new();
        buf.put_u8(b'Q');
        buf.put_i32(4 + query.len() as i32 + 1);
        buf.put_slice(query.as_bytes());
        buf.put_u8(0);

        let msg = parse_frontend_message(&mut buf).unwrap().unwrap();
        assert_eq!(msg, FrontendMessage::Query("SELECT 1".to_string()));
    }

    #[test]
    fn test_encode_query() {
        let encoded = encode_query("SELECT 1");
        let mut buf = encoded;
        let msg = parse_frontend_message(&mut buf).unwrap().unwrap();
        assert_eq!(msg, FrontendMessage::Query("SELECT 1".to_string()));
    }
}
