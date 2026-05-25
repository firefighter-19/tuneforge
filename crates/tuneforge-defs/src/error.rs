use thiserror::Error;

pub type DefResult<T> = Result<T, DefError>;

#[derive(Debug, Error)]
pub enum DefError {
    #[error("xml parse error: {0}")]
    Xml(#[from] quick_xml::DeError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("missing required field `{0}` after resolution")]
    MissingRequiredField(&'static str),

    #[error("unknown table type `{0}`")]
    UnknownTableType(String),

    #[error("unknown storage type `{0}`")]
    UnknownStorageType(String),

    #[error("invalid value for `{kind}`: `{value}`")]
    UnknownEnumValue { kind: &'static str, value: String },

    #[error("`{kind}` base `{name}` not found")]
    BaseNotFound { kind: &'static str, name: String },

    #[error("inheritance cycle detected in `{kind}` chain at `{name}`")]
    Cycle { kind: &'static str, name: String },

    #[error("invalid value for `{field}`: `{value}` ({reason})")]
    InvalidValue {
        field: &'static str,
        value: String,
        reason: String,
    },

    #[error("invalid scaling expression `{expr}`: {message}")]
    InvalidScaling { expr: String, message: String },

    #[error(transparent)]
    Core(#[from] tuneforge_core::CoreError),
}
