use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError {
    Unavailable {
        message: String,
        code: Option<String>,
    },
    InvalidRequest {
        message: String,
    },
    Dependency {
        message: String,
    },
}

impl ServiceError {
    pub fn code(&self) -> Option<&str> {
        match self {
            ServiceError::Unavailable { code, .. } => code.as_deref(),
            ServiceError::InvalidRequest { .. } | ServiceError::Dependency { .. } => None,
        }
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceError::Unavailable { message, .. }
            | ServiceError::InvalidRequest { message }
            | ServiceError::Dependency { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for ServiceError {}
