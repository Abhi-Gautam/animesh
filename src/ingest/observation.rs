use crate::ids::ReleaseKind;

#[derive(Debug, Clone, PartialEq)]
pub struct SourceObservation {
    pub source: String,
    pub source_id: String,
    pub raw_payload_id: String,
    pub kind: ReleaseKind,
    pub display_title: String,
    pub raw_title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub observed_at: i64,
    pub source_updated_at: Option<i64>,
    pub aliases: Vec<AliasObservation>,
    pub external_ids: Vec<ExternalIdObservation>,
    pub release_events: Vec<ReleaseEventObservation>,
    pub links: Vec<LinkObservation>,
    pub images: Vec<ImageObservation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AliasObservation {
    pub alias: String,
    pub locale: Option<String>,
    pub alias_kind: Option<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternalIdObservation {
    pub id_kind: String,
    pub id_value: String,
    pub confidence: f64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimePrecision {
    Instant,
    Date,
    Month,
    Year,
    Unknown,
}

impl TimePrecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Instant => "instant",
            Self::Date => "date",
            Self::Month => "month",
            Self::Year => "year",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseEventObservation {
    pub id: String,
    pub event_kind: String,
    pub title: Option<String>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub local_date: Option<String>,
    pub local_time: Option<String>,
    pub source_timezone: Option<String>,
    pub scheduled_at: Option<i64>,
    pub precision: TimePrecision,
    pub confidence: f64,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkObservation {
    pub site: String,
    pub url: String,
    pub link_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageObservation {
    pub image_kind: String,
    pub url: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
}
