use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError {
    Unavailable { message: String },
    InvalidRequest { message: String },
    Dependency { message: String },
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceError::Unavailable { message }
            | ServiceError::InvalidRequest { message }
            | ServiceError::Dependency { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for ServiceError {}
