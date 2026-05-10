use thiserror::Error;

pub type DefResult<T> = Result<T, DefError>;

#[derive(Debug, Error)]
pub enum DefError {
    #[error("xml parse error: {0}")]
    Xml(#[from] quick_xml::DeError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("missing required attribute `{0}`")]
    MissingAttribute(&'static str),

    #[error("unknown table type `{0}`")]
    UnknownTableType(String),

    #[error("invalid scaling expression `{expr}`: {message}")]
    InvalidScaling { expr: String, message: String },

    #[error(transparent)]
    Core(#[from] romraider_core::CoreError),
}
