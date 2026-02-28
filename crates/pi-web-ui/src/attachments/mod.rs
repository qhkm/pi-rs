use serde::{Deserialize, Serialize};

/// Supported attachment types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentType {
    Image,
    Pdf,
    Docx,
    Xlsx,
    Pptx,
    Text,
}

/// An attachment in a user message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub attachment_type: AttachmentType,
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extracted_text: Option<String>,
}
