//! One serialisable bundle of every model, for the server to persist as a single JSON blob.
use super::core::ModelCard;
use super::{fit_model::FitModel, jd_classifier::JdClassifier, outcome::OutcomeModel, pii_candidate::CandidateClassifier, ranker::BulletRanker, rewrite_need::RewriteNeed};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;

/// Every field defaults to its cold-start prior, so older/partial JSON still loads and unknown
/// fields from newer versions are ignored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelBundle {
    pub version: u32,
    pub fit: FitModel,
    pub ranker: BulletRanker,
    pub rewrite: RewriteNeed,
    pub jd: JdClassifier,
    pub outcome: OutcomeModel,
    pub pii: CandidateClassifier,
}
impl Default for ModelBundle {
    fn default() -> Self {
        ModelBundle { version: VERSION, fit: Default::default(), ranker: Default::default(), rewrite: Default::default(), jd: Default::default(), outcome: Default::default(), pii: Default::default() }
    }
}

impl ModelBundle {
    pub fn cards(&self) -> Vec<ModelCard> {
        let mut v = vec![self.fit.card(), self.ranker.card(), self.rewrite.card()];
        v.extend(self.jd.cards());
        v.push(self.outcome.card());
        v.push(self.pii.card());
        v
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("model bundle serialises")
    }
    /// Err only on malformed JSON; missing fields fall back to defaults.
    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ml::core::Status;

    #[test]
    fn round_trips_and_tolerates_partial_json() {
        let mut b = ModelBundle::default();
        b.jd.correct("rust embedded firmware rtos", "embedded", "mid");
        let back = ModelBundle::from_json(&b.to_json()).unwrap();
        assert_eq!(b, back);
        let cards = b.cards();
        assert_eq!(cards.len(), 7);
        assert!(cards.iter().all(|c| c.status == Status::ColdStart));
        let partial = ModelBundle::from_json(r#"{"version":9,"future_field":1,"pii":{"samples":[]}}"#).unwrap();
        assert_eq!(partial.version, 9);
        assert_eq!(partial.jd, JdClassifier::seed());
        assert!(ModelBundle::from_json("nope").is_err());
    }
}
