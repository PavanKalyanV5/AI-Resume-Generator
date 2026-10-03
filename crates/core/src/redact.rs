//! Reversible, fail-closed PII vault. Memory-only; never serialised.
use crate::schema::Resume;
use regex::Regex;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Person,
    Email,
    Phone,
    Org,
    Client,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Kind::Person => "PERSON",
            Kind::Email => "EMAIL",
            Kind::Phone => "PHONE",
            Kind::Org => "ORG",
            Kind::Client => "CLIENT",
        }
    }
}

struct Entry {
    kind: Kind,
    value: String,
    norm: String,
    token: String,
    re: Option<Regex>, // None for phones (digit matching)
}

pub struct Vault {
    entries: Vec<Entry>,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Vault({} entries)", self.entries.len())
    }
}

/// Names the kind and token only, never the value.
#[derive(Debug, PartialEq)]
pub struct Leak(pub Vec<(Kind, String)>);

impl fmt::Display for Leak {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v: Vec<String> = self.0.iter().map(|(k, t)| format!("{:?}:{}", k, t)).collect();
        write!(f, "payload leaks redacted values: {}", v.join(", "))
    }
}
impl std::error::Error for Leak {}

#[derive(Debug, PartialEq)]
pub enum RestoreError {
    UnknownToken(String),
    Residual(String),
}

impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RestoreError::UnknownToken(t) => write!(f, "unknown token {t}"),
            RestoreError::Residual(t) => write!(f, "token {t} remains after restore"),
        }
    }
}
impl std::error::Error for RestoreError {}

/// Entries with fewer normalised chars than this are guarded with word boundaries only.
const SHORT_ENTRY: usize = 6;

const SUFFIXES: &[&str] = &[
    "pvt", "ltd", "inc", "llc", "corp", "limited", "technologies", "technology",
];

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

fn digits(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn tail10(d: &str) -> &str {
    &d[d.len().saturating_sub(10)..]
}

fn words(s: &str) -> Vec<&str> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect()
}

fn token_re() -> Regex {
    Regex::new(r"\[(?:PERSON|EMAIL|PHONE|ORG|CLIENT)_\d+\]").unwrap()
}

fn phone_re() -> Regex {
    Regex::new(r"\+?\d[\d\s().-]{6,}\d").unwrap()
}

impl Vault {
    pub fn from_resume(r: &Resume, extra: &[(Kind, String)]) -> Vault {
        let mut raw: Vec<(Kind, String)> = Vec::new();
        let name = r.profile.name.trim();
        if !name.is_empty() {
            raw.push((Kind::Person, name.to_string()));
            for w in words(name).into_iter().filter(|w| w.chars().count() >= 3) {
                raw.push((Kind::Person, w.to_string()));
            }
        }
        let email = r.profile.email.trim();
        if !email.is_empty() {
            raw.push((Kind::Email, email.to_string()));
            if let Some((local, _)) = email.split_once('@') {
                if norm(local).len() >= 4 {
                    raw.push((Kind::Email, local.to_string()));
                }
            }
        }
        if let Some(p) = &r.profile.phone {
            raw.push((Kind::Phone, p.trim().to_string()));
        }
        for e in &r.experience {
            raw.push((Kind::Org, e.organization.trim().to_string()));
            if let Some(c) = &e.client {
                raw.push((Kind::Client, c.trim().to_string()));
            }
        }
        raw.extend(extra.iter().map(|(k, v)| (*k, v.trim().to_string())));

        // Org/Client legal-suffix-stripped variants.
        let mut stripped = Vec::new();
        for (k, v) in &raw {
            if matches!(k, Kind::Org | Kind::Client) {
                let mut w = words(v);
                while w.len() > 1 && SUFFIXES.contains(&w[w.len() - 1].to_lowercase().as_str()) {
                    w.pop();
                }
                let s = w.join(" ");
                if s != *v && norm(&s).len() >= 3 {
                    stripped.push((*k, s));
                }
            }
        }
        raw.extend(stripped);

        let mut entries: Vec<Entry> = Vec::new();
        for (kind, value) in raw {
            let n = if kind == Kind::Phone { digits(&value) } else { norm(&value) };
            let min = if kind == Kind::Phone { 7 } else { 3 };
            if n.len() < min || entries.iter().any(|e| e.kind == kind && e.norm == n) {
                continue;
            }
            let re = match kind {
                Kind::Phone => None,
                // emails: literal separators so local-part never eats "Jane Doe"
                Kind::Email => Some(format!(r"(?i)\b{}\b", regex::escape(&value))),
                _ => {
                    let parts: Vec<String> = words(&value).iter().map(|w| regex::escape(w)).collect();
                    Some(format!(r"(?i)\b{}\b", parts.join(r"[\W_]+")))
                }
            }
            .map(|p| Regex::new(&p).unwrap());
            entries.push(Entry { kind, value, norm: n, token: String::new(), re });
        }
        // Deterministic numbering: sort by (kind, normalized value).
        entries.sort_by(|a, b| (a.kind, &a.norm).cmp(&(b.kind, &b.norm)));
        let mut counts = std::collections::HashMap::new();
        for e in entries.iter_mut() {
            let c = counts.entry(e.kind).or_insert(0);
            *c += 1;
            e.token = format!("[{}_{}]", e.kind.tag(), c);
        }
        Vault { entries }
    }

    /// Read-only (kind, token, value) view for the PII inspector. Values are real: mask before they leave the process.
    pub fn entries_view(&self) -> Vec<(Kind, String, String)> {
        self.entries.iter().map(|e| (e.kind, e.token.clone(), e.value.clone())).collect()
    }

    pub fn redact(&self, text: &str) -> String {
        // (start, end, replacement, is_existing_token)
        let mut cands: Vec<(usize, usize, &str, bool)> = Vec::new();
        for m in token_re().find_iter(text) {
            cands.push((m.start(), m.end(), m.as_str(), true));
        }
        // Emails first so an equal-length literal email local-part beats a flexible name match.
        let order = self.entries.iter().filter(|e| e.kind == Kind::Email).chain(self.entries.iter().filter(|e| e.kind != Kind::Email));
        for e in order {
            match &e.re {
                Some(re) => cands.extend(re.find_iter(text).map(|m| (m.start(), m.end(), e.token.as_str(), false))),
                None => {
                    for m in phone_re().find_iter(text) {
                        let d = digits(m.as_str());
                        if tail10(&d) == tail10(&e.norm) {
                            cands.push((m.start(), m.end(), e.token.as_str(), false));
                        }
                    }
                }
            }
        }
        cands.sort_by_key(|c| (!c.3, std::cmp::Reverse(c.1 - c.0), c.0));
        let mut kept: Vec<(usize, usize, &str, bool)> = Vec::new();
        for c in cands {
            if kept.iter().all(|k| c.1 <= k.0 || c.0 >= k.1) {
                kept.push(c);
            }
        }
        kept.sort_by_key(|c| c.0);
        let mut out = String::with_capacity(text.len());
        let mut pos = 0;
        for (s, e, rep, _) in kept {
            out.push_str(&text[pos..s]);
            out.push_str(rep);
            pos = e;
        }
        out.push_str(&text[pos..]);
        out
    }

    pub fn guard(&self, payload: &str) -> Result<(), Leak> {
        let n = norm(payload);
        let runs: Vec<String> = phone_re().find_iter(payload).map(|m| digits(m.as_str())).collect();
        let leaks: Vec<(Kind, String)> = self
            .entries
            .iter()
            .filter(|e| {
                if e.kind == Kind::Phone {
                    runs.iter().any(|d| tail10(d) == tail10(&e.norm))
                } else if e.norm.len() < SHORT_ENTRY {
                    // Short fragments ("doe") would false-positive inside normal words, so
                    // require word boundaries; longer values keep the obfuscation-proof match.
                    e.re.as_ref().is_some_and(|re| re.is_match(payload))
                } else {
                    n.contains(&e.norm)
                }
            })
            .map(|e| (e.kind, e.token.clone()))
            .collect();
        if leaks.is_empty() { Ok(()) } else { Err(Leak(leaks)) }
    }

    pub fn restore(&self, text: &str) -> Result<String, RestoreError> {
        let re = token_re();
        let mut out = String::with_capacity(text.len());
        let mut pos = 0;
        for m in re.find_iter(text) {
            let e = self
                .entries
                .iter()
                .find(|e| e.token == m.as_str())
                .ok_or_else(|| RestoreError::UnknownToken(m.as_str().to_string()))?;
            out.push_str(&text[pos..m.start()]);
            out.push_str(&e.value);
            pos = m.end();
        }
        out.push_str(&text[pos..]);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Experience;

    fn resume() -> Resume {
        let mut r = Resume::default();
        r.profile.name = "Jane Doe".into();
        r.profile.email = "jane.doe@example.com".into();
        r.profile.phone = Some("+1 555 010 1234".into());
        r.experience.push(Experience {
            organization: "Acme Robotics Pvt Ltd".into(),
            client: Some("Globex Corp".into()),
            ..Default::default()
        });
        r
    }

    const TEXT: &str = "Jane Doe (Jane) at Acme Robotics Pvt Ltd built X for Globex Corp. Mail jane.doe@example.com or jane.doe, Doe.";

    #[test]
    fn round_trip_and_guard() {
        let v = Vault::from_resume(&resume(), &[]);
        assert!(v.guard(TEXT).is_err());
        let red = v.redact(TEXT);
        assert!(v.guard(&red).is_ok(), "{red}");
        assert!(!red.contains("Jane") && !red.contains("Acme") && !red.contains("Globex"));
        assert_eq!(v.restore(&red).unwrap(), TEXT);
    }

    #[test]
    fn phones_and_case_flex() {
        let v = Vault::from_resume(&resume(), &[]);
        let t = "call +1 (555) 010-1234 or 555.010.1234 or 5550101234; acme  robotics, Acme-Robotics, GLOBEX corp";
        let red = v.redact(t);
        assert_eq!(red.matches("[PHONE_1]").count(), 3, "{red}");
        assert!(v.guard(&red).is_ok(), "{red}");
        assert!(v.guard(t).is_err());
        assert_eq!(red.matches("[ORG_").count(), 2);
        assert_eq!(red.matches("[CLIENT_").count(), 1);
    }

    #[test]
    fn idempotent_and_no_double_redact() {
        let v = Vault::from_resume(&resume(), &[]);
        let once = v.redact(TEXT);
        assert_eq!(v.redact(&once), once);
        assert_eq!(v.redact("x [PERSON_1] y"), "x [PERSON_1] y");
    }

    #[test]
    fn unknown_token_rejected() {
        let v = Vault::from_resume(&resume(), &[]);
        assert!(matches!(v.restore("hi [PERSON_99]"), Err(RestoreError::UnknownToken(_))));
        assert!(v.restore("no tokens").is_ok());
    }

    #[test]
    fn deterministic() {
        let a = Vault::from_resume(&resume(), &[(Kind::Org, "Zeta Labs".into())]);
        let b = Vault::from_resume(&resume(), &[(Kind::Org, "Zeta Labs".into())]);
        assert_eq!(a.redact(TEXT), b.redact(TEXT));
        let toks = |v: &Vault| v.entries.iter().map(|e| (e.token.clone(), e.value.clone())).collect::<Vec<_>>();
        assert_eq!(toks(&a), toks(&b));
    }

    #[test]
    fn word_boundaries() {
        let v = Vault::from_resume(&resume(), &[]);
        assert_eq!(v.redact("a doer named janet"), "a doer named janet");
    }

    #[test]
    fn guard_ignores_short_fragments_inside_words() {
        let v = Vault::from_resume(&resume(), &[]);
        assert!(v.guard("it does what a doer does").is_ok());
        assert!(v.guard("ask Doe about it").is_err());
        assert!(v.guard("Acme-Robotics shipped").is_err()); // long values stay obfuscation-proof
    }

    #[test]
    fn debug_hides_values() {
        let v = Vault::from_resume(&resume(), &[]);
        assert!(!format!("{v:?}").contains("Jane"));
    }
}
