//! Error types for NIMon

use thiserror::Error;

#[derive(Error, Debug)]
pub enum NimonError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("NI API error in {api}: status {code}")]
    NiApi { api: &'static str, code: i32 },

    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    #[error("Edge node not found: {0}")]
    EdgeNotFound(String),

    #[error("Alert not found: {0}")]
    AlertNotFound(String),

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Timeout waiting for {0}")]
    Timeout(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type NimonResult<T> = Result<T, NimonError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = NimonError::DeviceNotFound("PXI1Slot2".to_string());
        assert!(err.to_string().contains("PXI1Slot2"));
    }

    #[test]
    fn test_ni_api_error() {
        let err = NimonError::NiApi {
            api: "NiSysCfg_Initialize",
            code: -1,
        };
        let msg = err.to_string();
        assert!(msg.contains("NiSysCfg_Initialize"));
        assert!(msg.contains("-1"));
    }
}
