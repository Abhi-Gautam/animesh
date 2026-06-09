#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct SourceRequest {
    pub source: String,
    pub endpoint: String,
    pub method: HttpMethod,
    pub request_key: String,
    pub request_hash: String,
    pub request_json: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawSourcePayload {
    pub id: String,
    pub source: String,
    pub endpoint: String,
    pub method: HttpMethod,
    pub request_key: String,
    pub request_hash: String,
    pub request_json: Option<String>,
    pub http_status: i64,
    pub response_hash: String,
    pub response_json: String,
    pub fetched_at: i64,
    pub expires_at: Option<i64>,
    pub created_at: i64,
}
