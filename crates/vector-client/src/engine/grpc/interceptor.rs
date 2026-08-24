//! gRPC api-key metadata injection.
//!
//! tonic-generated clients accept no interceptors, so authentication is
//! applied per request: [`parse_api_key`] validates the configured key once
//! at connect time — invalid values fail startup loudly instead of silently
//! disabling auth at runtime — and every outgoing RPC goes through
//! [`attach_api_key`], which inserts the key into the request metadata.
//! This mirrors the HTTP engine, which sets the same header on each call.

use tonic::metadata::{Ascii, MetadataValue};
use tonic::Request;

/// Metadata/header name carrying the API key on every request.
pub const API_KEY_HEADER: &str = "api-key";

/// Parse an API key into valid header metadata.
pub fn parse_api_key(key: &str) -> Result<MetadataValue<Ascii>, String> {
    MetadataValue::try_from(key).map_err(|e| format!("invalid connection.api_key: {}", e))
}

/// Insert the API key into an outgoing request's metadata.
pub fn attach_api_key<T>(mut request: Request<T>, api_key: &MetadataValue<Ascii>) -> Request<T> {
    request
        .metadata_mut()
        .insert(API_KEY_HEADER, api_key.clone());
    request
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_api_key_accepts_valid_values() {
        assert!(parse_api_key("secret-key").is_ok());
        assert!(parse_api_key("abc123_-").is_ok());
    }

    #[test]
    fn test_parse_api_key_rejects_invalid_metadata_characters() {
        // Control characters cannot appear in HTTP/2 header values.
        assert!(parse_api_key("bad\nkey").is_err());
    }

    #[test]
    fn test_attach_api_key_inserts_metadata() {
        let key = parse_api_key("secret").expect("valid key");
        let request = attach_api_key(Request::new(()), &key);
        let value = request.metadata().get(API_KEY_HEADER);
        assert_eq!(value, Some(&key));
    }
}
