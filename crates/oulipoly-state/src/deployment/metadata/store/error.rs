use crate::deployment::metadata::schema::SchemaError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetadataError {
    NotFound,
    Corrupt(String),
    Sql(String),
}

impl From<rusqlite::Error> for MetadataError {
    fn from(err: rusqlite::Error) -> Self {
        MetadataError::Sql(err.to_string())
    }
}

impl From<std::io::Error> for MetadataError {
    fn from(err: std::io::Error) -> Self {
        MetadataError::Sql(err.to_string())
    }
}

impl From<chrono::ParseError> for MetadataError {
    fn from(err: chrono::ParseError) -> Self {
        MetadataError::Corrupt(err.to_string())
    }
}

impl From<SchemaError> for MetadataError {
    fn from(err: SchemaError) -> Self {
        match err {
            SchemaError::Sql(message) => MetadataError::Sql(message),
        }
    }
}

pub(super) fn metadata_open_error(err: rusqlite::Error) -> MetadataError {
    if matches!(err, rusqlite::Error::SqliteFailure(_, _)) {
        MetadataError::Corrupt(err.to_string())
    } else {
        MetadataError::from(err)
    }
}
