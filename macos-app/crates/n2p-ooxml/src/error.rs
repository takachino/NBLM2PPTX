//! 錯誤型別。

/// 所有 crate 內錯誤。刻意保留來源描述，方便對照 OOXML 規格排查。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid ZIP container: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("XML parsing failed: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("XML escaping failed: {0}")]
    Escape(#[from] quick_xml::escape::EscapeError),

    #[error("Part not found: {0}")]
    PartNotFound(String),

    #[error("Required node not found: {0}")]
    NodeNotFound(String),

    #[error("Unexpected structure: {0}")]
    UnexpectedStructure(String),

    #[error("Value out of range: {0}")]
    OutOfRange(String),
}

pub type Result<T> = std::result::Result<T, Error>;
