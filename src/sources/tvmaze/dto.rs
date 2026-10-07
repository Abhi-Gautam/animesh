//! TVmaze wire shapes. Only this module deserializes TVmaze JSON.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SearchHit {
    pub show: Show,
}

#[derive(Debug, Deserialize)]
pub struct Show {
    pub id: Option<i64>,
    pub name: Option<String>,
    pub status: Option<String>,
    #[serde(rename = "type")]
    pub show_type: Option<String>,
    pub premiered: Option<String>,
    #[serde(rename = "_embedded")]
    pub embedded: Option<Embedded>,
}

#[derive(Debug, Deserialize)]
pub struct Embedded {
    pub nextepisode: Option<Episode>,
}

#[derive(Debug, Deserialize)]
pub struct Episode {
    pub season: Option<i32>,
    pub number: Option<i64>,
    pub airstamp: Option<String>,
}
