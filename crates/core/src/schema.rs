//! Canonical resume model. Importers produce it, renderers consume it.
//! Unknown input fields are ignored; everything optional defaults to empty.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Resume {
    pub profile: Profile,
    pub socials: Vec<Link>,
    pub summary: Vec<String>,
    pub experience: Vec<Experience>,
    pub education: Vec<Education>,
    pub skills: Vec<SkillCategory>,
    pub projects: Vec<Project>,
    pub certifications: Vec<Certification>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    pub name: String,
    pub email: String,
    pub phone: Option<String>,
    pub role: String,
    pub location: String,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Link {
    pub label: String,
    pub url: String,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Experience {
    pub id: String,
    pub role: String,
    pub organization: String,
    /// End client when the employer is a consultancy; redacted like an org.
    pub client: Option<String>,
    pub location: String,
    pub date_label: String,
    pub bullets: Vec<String>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Education {
    pub institution: String,
    pub credential: String,
    pub grade: String,
    pub date_label: String,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct SkillCategory {
    pub label: String,
    pub skills: Vec<String>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    pub name: String,
    pub date_label: String,
    pub tech_stack: Vec<String>,
    pub description: String,
    pub bullets: Vec<String>,
    pub links: Vec<Link>,
    /// Owner-marked highlight (tier prior in selection).
    pub featured: bool,
    /// Imported data often marks highlights as `tier: "featured"`; see `is_featured`.
    pub tier: Option<String>,
}

impl Project {
    pub fn is_featured(&self) -> bool {
        self.featured || self.tier.as_deref().is_some_and(|t| t.eq_ignore_ascii_case("featured"))
    }
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Certification {
    pub title: String,
    pub issuer: String,
    pub date_label: String,
    pub verification_url: Option<String>,
    pub featured: bool,
}

impl Resume {
    /// Experience, education, projects and certifications newest first (current first, then end, then start; see
    /// `seniority::Interval`). Unknown dates last, stable.
    pub fn sort_reverse_chrono(&mut self) {
        let (now, key) = (crate::seniority::now_month(), crate::seniority::recency_key);
        self.experience.sort_by_cached_key(|e| std::cmp::Reverse(key(&e.date_label, now)));
        self.education.sort_by_cached_key(|e| std::cmp::Reverse(key(&e.date_label, now)));
        self.projects.sort_by_cached_key(|p| std::cmp::Reverse(key(&p.date_label, now)));
        self.certifications.sort_by_cached_key(|c| std::cmp::Reverse(key(&c.date_label, now)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_featured_counts_as_featured() {
        let p: Project = serde_json::from_str(r#"{"name":"A","tier":"featured"}"#).unwrap();
        assert!(p.is_featured());
        let q: Project = serde_json::from_str(r#"{"name":"B","tier":"compact"}"#).unwrap();
        assert!(!q.is_featured());
    }
}
