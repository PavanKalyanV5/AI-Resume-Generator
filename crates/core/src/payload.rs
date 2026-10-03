//! The only thing ever sent to an LLM: redacted, link-free, id-addressed text.
use crate::jd::{find_terms, Jd, ReqKind};
use crate::select::weight_of;
use crate::seniority::experience_years;
use crate::redact::{Leak, Vault};
use crate::grounding::{apply_grounding_with, parse_repair, GroundingEvent};
use crate::profile_model::{Level, ProfileFacts};
use crate::schema::Resume;
use crate::select::Selection;
use anyhow::{bail, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Debug)]
pub struct Payload {
    pub system: String,
    /// JSON document.
    pub user: String,
}

const SYSTEM: &str = "You rewrite resume text to better match a job description. \
Keep every fact true: never add employers, tools, numbers or claims that are not in the input. \
Tokens like [ORG_1] or [PERSON_1] are opaque placeholders: copy them unchanged, never invent new ones. \
Reply with ONLY a JSON object of this shape: \
{\"summary\": [string], \"bullets\": [{\"id\": string, \"text\": string}]}. \
Each bullets.id must be one of the ids given in the input; omit bullets you leave unchanged. \
summary is REQUIRED: exactly `summary_lines` bullets, each 22-32 words, tailored to the job and built only from facts in the input. \
Bullet 1 has this exact shape: `<role label> with **N+ years** of experience <building/doing> **A**, **B** and **C**`, where the role label is exactly the given `role_label`, copied verbatim, nothing after it (no team, org or qualifier), \
N+ years is the years phrase `years_phrase` copied exactly (never claim more; if it is null write no years at all) and A, B, C are the top job-matched skills the input supports. \
Bullets 2+ are high-level capability and outcome statements: what the candidate can do and the result it brings, with 2-4 bold key terms. \
No low-level implementation detail and never copy an input bullet. Good: \"Delivers **reliable** backend platforms that keep **document workflows** fast and observable for growing product teams.\" \
Bad: \"Engineered an asynchronous pipeline across API, worker and UI running as five containerized services.\" \
In every summary bullet and every rewritten bullet wrap the key terms in **double asterisks** for bold. \
No invented facts, numbers or tools; no company names or tokens other than those provided.";

/// Addressable (id, original text): `e{exp}.b{n}`, `p{proj}.d`, `p{proj}.b{n}`.
pub(crate) fn items(r: &Resume, sel: &Selection) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for (i, idx) in sel.bullets.iter().enumerate() {
        for &j in idx {
            if let Some(b) = r.experience.get(i).and_then(|e| e.bullets.get(j)) {
                v.push((format!("e{i}.b{j}"), b.clone()));
            }
        }
    }
    for &p in &sel.projects {
        if let Some(pr) = r.projects.get(p) {
            if !pr.description.trim().is_empty() {
                v.push((format!("p{p}.d"), pr.description.clone()));
            }
            v.extend(pr.bullets.iter().enumerate().map(|(j, b)| (format!("p{p}.b{j}"), b.clone())));
        }
    }
    v
}

/// Mutable text slot addressed by an `items` id.
pub(crate) fn slot_mut<'a>(r: &'a mut Resume, id: &str) -> Option<&'a mut String> {
    let (head, tail) = id.split_once('.')?;
    let n: usize = head.get(1..)?.parse().ok()?;
    match head.as_bytes().first()? {
        b'e' => r.experience.get_mut(n)?.bullets.get_mut(tail.get(1..)?.parse::<usize>().ok()?),
        _ if tail.starts_with('d') => Some(&mut r.projects.get_mut(n)?.description),
        _ => r.projects.get_mut(n)?.bullets.get_mut(tail.get(1..)?.parse::<usize>().ok()?),
    }
}

fn strip_urls(s: &str) -> String {
    Regex::new(r"(?i)\b(?:https?://|www\.)\S+").unwrap().replace_all(s, "").trim().to_string()
}

pub fn build_payload(r: &Resume, sel: &Selection, jd: &Jd, vault: &Vault) -> Result<Payload, Leak> {
    let red = |s: &str| vault.redact(&strip_urls(s));
    let reqs: Vec<_> = jd.requirements.iter().take(20).map(|q| json!({"term": red(&q.term), "required": q.required})).collect();
    let bullets: Vec<_> = items(r, sel).iter().map(|(id, t)| json!({"id": id, "text": red(t)})).collect();
    let skills: Vec<_> = sel
        .skills
        .iter()
        .filter_map(|(ci, idx)| {
            let c = r.skills.get(*ci)?;
            Some(json!({"label": red(&c.label), "skills": idx.iter().filter_map(|&k| c.skills.get(k)).map(|s| red(s)).collect::<Vec<_>>()}))
        })
        .collect();
    let years = experience_years(r).floor();
    let user = json!({
        "job": {"role_label": red(&short_role(jd).unwrap_or_else(|| r.profile.role.trim().to_string())), "requirements": reqs},
        "summary": r.summary.iter().map(|s| red(s)).collect::<Vec<_>>(),
        "summary_lines": if bullets.len() >= 12 { 4 } else { 3 },
        "experience_years": years,
        "years_phrase": (years >= 1.0).then(|| format!("{}+ years", years as u32)),
        "bullets": bullets,
        "skills": skills,
    })
    .to_string();
    let p = Payload { system: SYSTEM.to_string(), user };
    vault.guard(&format!("{}\n{}", p.system, p.user))?;
    Ok(p)
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    summary: Vec<String>,
    #[serde(default)]
    bullets: Vec<ReplyBullet>,
}

#[derive(Deserialize)]
struct ReplyBullet {
    id: String,
    text: String,
}

/// Parse the model reply, validate ids, restore tokens, return the trimmed resume with rewrites applied.
pub fn parse_reply(reply: &str, vault: &Vault, original: &Resume, sel: &Selection) -> Result<Resume> {
    Ok(sel.apply(&parse_reply_full(reply, vault, original, sel)?))
}

/// Like `parse_reply` but returns the full (un-trimmed) resume with rewrites in place, for `fit::fit_to_pages`.
/// Fails when `summary` is missing or empty.
pub fn parse_reply_full(reply: &str, vault: &Vault, original: &Resume, sel: &Selection) -> Result<Resume> {
    let (Some(a), Some(b)) = (reply.find('{'), reply.rfind('}')) else { bail!("no JSON object in reply") };
    let parsed: Reply = serde_json::from_str(&reply[a..=b])?;
    if parsed.summary.iter().all(|s| s.trim().is_empty()) {
        bail!("reply is missing the required non-empty `summary` array");
    }
    let valid: Vec<String> = items(original, sel).into_iter().map(|(id, _)| id).collect();
    let mut out = original.clone();
    for rb in &parsed.bullets {
        if !valid.contains(&rb.id) {
            bail!("reply references unknown id {}", rb.id);
        }
        let text = vault.restore(&rb.text)?;
        let (head, tail) = rb.id.split_once('.').expect("validated id");
        let n: usize = head[1..].parse()?;
        match (head.as_bytes()[0], &tail[..1]) {
            (b'e', _) => out.experience[n].bullets[tail[1..].parse::<usize>()?] = text,
            (_, "d") => out.projects[n].description = text,
            _ => out.projects[n].bullets[tail[1..].parse::<usize>()?] = text,
        }
    }
    out.summary = parsed.summary.iter().filter(|s| !s.trim().is_empty()).map(|s| vault.restore(s)).collect::<Result<_, _>>()?;
    Ok(out)
}

/// `parse_reply_full` plus the grounding guard: violating rewrites are reverted. Returns the full resume.
pub fn parse_reply_grounded_full(reply: &str, vault: &Vault, original: &Resume, sel: &Selection, facts: &ProfileFacts, jd: &Jd) -> Result<(Resume, Vec<GroundingEvent>)> {
    parse_reply_grounded_repaired(reply, None, vault, original, sel, facts, jd)
}

/// Grounded parse that keeps lines fixed by the repair reply (`grounding::repair_prompt` call) when they pass the checks.
pub fn parse_reply_grounded_repaired(reply: &str, repair: Option<&str>, vault: &Vault, original: &Resume, sel: &Selection, facts: &ProfileFacts, jd: &Jd) -> Result<(Resume, Vec<GroundingEvent>)> {
    let full = parse_reply_full(reply, vault, original, sel)?;
    let repairs = repair.map(|r| parse_repair(r, vault)).unwrap_or_default();
    Ok(apply_grounding_with(original, full, sel, facts, jd, &repairs))
}

/// Like `parse_reply` (trimmed to the selection) with the grounding guard applied.
pub fn parse_reply_grounded(reply: &str, vault: &Vault, original: &Resume, sel: &Selection, facts: &ProfileFacts, jd: &Jd) -> Result<(Resume, Vec<GroundingEvent>)> {
    let (r, ev) = parse_reply_grounded_full(reply, vault, original, sel, facts, jd)?;
    Ok((sel.apply(&r), ev))
}

/// The only years claim line 1 may carry: `N+ years` from the profile facts, None under one year.
pub fn years_phrase(facts: &ProfileFacts) -> Option<String> {
    let y = facts.years.total_effective.floor() as u32;
    (y >= 1).then(|| format!("{y}+ years"))
}

fn short_role(jd: &Jd) -> Option<String> {
    let full = jd.role.as_deref().or(jd.title.as_deref()).unwrap_or("");
    let cut = [",", " - ", " – ", " | ", " (", " for ", " at ", " with "].iter().filter_map(|d| full.find(d)).min().unwrap_or(full.len());
    Some(full[..cut].split_whitespace().take(5).collect::<Vec<_>>().join(" ")).filter(|s| !s.is_empty())
}

/// Short role label for line 1: the JD title cut at the first `, - – | (` or ` for/at/with`, at most 5 words; else the headline.
pub fn role_label(facts: &ProfileFacts, jd: &Jd) -> String {
    if let Some(s) = short_role(jd) { s } else if facts.headline.trim().is_empty() { "Software Engineer".into() } else { facts.headline.trim().into() }
}

/// Deterministic summary line 1 (no AI): `<role> with **N+ years** of experience building **A**, **B**, and **C**.`
/// Skills: up to 4 JD-matched (by JD weight, then proficiency), topped up with the most proficient to reach 3.
pub fn line1_template(facts: &ProfileFacts, jd: &Jd) -> String {
    let mut s = role_label(facts, jd);
    if let Some(y) = years_phrase(facts) {
        s += &format!(" with **{y}** of experience");
    }
    let w = |c: &str| jd.requirements.iter().filter(|q| q.kind == ReqKind::Skill && q.term == c).map(|q| q.weight).fold(-1.0, f32::max);
    let mut ok: Vec<_> = facts.skills.iter().filter(|s| s.level >= Level::Working).collect();
    ok.sort_by(|a, b| (w(&b.canonical), b.proficiency).partial_cmp(&(w(&a.canonical), a.proficiency)).unwrap_or(std::cmp::Ordering::Equal));
    let matched = ok.iter().filter(|s| w(&s.canonical) >= 0.0).count();
    let sk: Vec<String> = ok.iter().take(matched.clamp(3, 4)).map(|s| format!("**{}**", facts.display(&s.canonical))).collect();
    match sk.split_last() {
        None => {}
        Some((l, [])) => s += &format!(" building {l}"),
        Some((l, [a])) => s += &format!(" building {a} and {l}"),
        Some((l, rest)) => s += &format!(" building {}, and {l}", rest.join(", ")),
    }
    s + "."
}

/// Facts-based fallback summary: years from the profile model and the JD skills the candidate is
/// most proficient in (best first), plus one achievement clause from the best JD-weighted bullet.
/// `role` overrides the JD-derived role (the job's known role); skills use the profile's own casing.
pub fn fallback_summary_with(facts: &ProfileFacts, jd: &Jd, role: Option<&str>) -> Vec<String> {
    let role = role.or(jd.role.as_deref()).or(jd.title.as_deref()).unwrap_or("software engineering");
    let me = if facts.headline.trim().is_empty() { "Engineer" } else { facts.headline.trim() };
    let years = facts.years.total_effective.floor() as u32;
    let mut out = vec![if years >= 1 {
        format!("{me} with {years}+ years of professional experience, targeting {role} roles.")
    } else {
        format!("Early-career {me} targeting {role} roles.")
    }];
    let skills: Vec<String> = facts
        .skills
        .iter()
        .filter(|s| s.level >= Level::Working && jd.requirements.iter().any(|q| q.kind == ReqKind::Skill && q.term == s.canonical))
        .take(4)
        .map(|s| facts.display(&s.canonical))
        .collect();
    if !skills.is_empty() {
        out.push(format!("Hands-on experience with {}.", skills.join(", ")));
    }
    let best = facts.achievements.iter().filter(|a| !a.text.contains("http")).fold(None, |best: Option<(f32, f32, &str)>, a| {
        let k = (weight_of(&a.text, jd), a.impact_score);
        if best.is_none_or(|b| k.0 > b.0 || (k.0 == b.0 && k.1 > b.1)) { Some((k.0, k.1, a.text.as_str())) } else { best }
    });
    if let Some((_, _, b)) = best {
        let clause: Vec<&str> = b.split([',', ';']).next().unwrap_or("").split_whitespace().take_while(|w| !w.chars().any(|c| c.is_ascii_digit())).collect();
        if clause.len() >= 3 {
            out.push(format!("Track record includes work such as: {}.", clause.join(" ").trim_end_matches('.')));
        }
    }
    out
}

/// Deterministic summary for when the AI is unavailable: role/years, top matched JD skills,
/// and one achievement clause from the best-scoring bullet (cut before any number).
pub fn fallback_summary(r: &Resume, jd: &Jd) -> Vec<String> {
    let role = jd.role.as_deref().or(jd.title.as_deref()).unwrap_or("software engineering");
    let me = if r.profile.role.trim().is_empty() { "Engineer" } else { r.profile.role.trim() };
    let years = experience_years(r).floor() as u32;
    let mut out = vec![if years >= 1 {
        format!("{me} with {years}+ years of professional experience, targeting {role} roles.")
    } else {
        format!("Early-career {me} targeting {role} roles.")
    }];
    let mut text = r.experience.iter().flat_map(|e| e.bullets.iter().map(String::as_str).chain([e.role.as_str()])).collect::<Vec<_>>().join("\n");
    text += &r.skills.iter().flat_map(|c| c.skills.iter()).fold(String::new(), |a, s| a + "\n" + s);
    text += &r.projects.iter().fold(String::new(), |a, p| format!("{a}\n{} {}", p.tech_stack.join(" "), p.bullets.join(" ")));
    let have = find_terms(&text);
    let skills: Vec<&str> = jd.requirements.iter().filter(|q| q.kind == ReqKind::Skill && have.contains_key(&q.term)).take(4).map(|q| q.term.as_str()).collect();
    if !skills.is_empty() {
        out.push(format!("Hands-on experience with {}.", skills.join(", ")));
    }
    let best = r.experience.iter().flat_map(|e| &e.bullets).filter(|b| !b.contains("http")).fold(None, |best: Option<(f32, &String)>, b| {
        let w = weight_of(b, jd);
        if best.is_none_or(|(bw, _)| w > bw) { Some((w, b)) } else { best }
    });
    if let Some((_, b)) = best {
        let clause: Vec<&str> = b.split([',', ';']).next().unwrap_or("").split_whitespace().take_while(|w| !w.chars().any(|c| c.is_ascii_digit())).collect();
        if clause.len() >= 3 {
            out.push(format!("Track record includes work such as: {}.", clause.join(" ").trim_end_matches('.')));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jd::parse_jd;
    use crate::select::testutil::{fake, FAKE_JD};
    use crate::select::{select, select_with_facts, Budget};

    #[test]
    fn short_role_and_multi_skill_line1() {
        let (r, mut jd) = (fake(), parse_jd(FAKE_JD));
        let f = crate::profile_model::build_facts(&r, crate::profile_model::fixture::NOW);
        jd.role = Some("Software Development Engineer, Seller and AM GenAI Tools".into());
        assert_eq!(role_label(&f, &jd), "Software Development Engineer");
        jd.role = Some("Senior Staff Backend Platform Engineer Foo Bar - Acme".into());
        assert_eq!(role_label(&f, &jd), "Senior Staff Backend Platform Engineer");
        jd.role = Some("Data Engineer (Remote)".into());
        assert_eq!(role_label(&f, &jd), "Data Engineer");
        jd.role = None;
        jd.title = None;
        assert_eq!(role_label(&f, &jd), f.headline.trim());
        let (r2, mut jd2) = (fake(), parse_jd(FAKE_JD));
        jd2.role = jd.role.clone().or(Some("Software Development Engineer, Seller and AM GenAI Tools".into()));
        let pl = build_payload(&r2, &Selection::default(), &jd2, &Vault::from_resume(&r2, &[])).unwrap();
        assert!(pl.user.contains(r#""role_label":"Software Development Engineer""#) && !pl.user.contains("Seller") && pl.system.contains("nothing after it"));
        let l = line1_template(&f, &parse_jd(FAKE_JD));
        assert!(l.matches("**").count() >= 6 && l.contains(", and **"), "{l}");
        assert!(l.contains("**Python**") && !l.contains("**python**"), "{l}");
    }

    fn setup() -> (Resume, Jd, Selection, Vault) {
        let (r, jd) = (fake(), parse_jd(FAKE_JD));
        let sel = select(&r, &jd, Budget::default());
        let v = Vault::from_resume(&r, &[]);
        (r, jd, sel, v)
    }

    #[test]
    fn payload_is_clean() {
        let (r, jd, sel, v) = setup();
        let p = build_payload(&r, &sel, &jd, &v).unwrap();
        let all = format!("{}{}", p.system, p.user);
        assert!(v.guard(&all).is_ok());
        for bad in ["Jane", "Doe", "Acme", "Globex", "Initech", "PII:", "http", "www.", "exp-secret", "example.com", "Springfield", "verify", "Amazon"] {
            assert!(!all.contains(bad), "leaked {bad}");
        }
        assert!(p.user.contains("[ORG_") && p.user.contains("[CLIENT_"));
        assert!(p.user.contains("e0.b1") && p.user.contains("p1.d") && p.user.contains("kubernetes"));
        assert!(serde_json::from_str::<serde_json::Value>(&p.user).is_ok());
    }

    #[test]
    fn reply_round_trip() {
        let (r, jd, sel, v) = setup();
        let p = build_payload(&r, &sel, &jd, &v).unwrap();
        let org = p.user.split("[ORG_").nth(1).map(|s| format!("[ORG_{}]", s.split(']').next().unwrap())).unwrap();
        let reply = format!("Sure!\n```json\n{{\"summary\":[\"Backend engineer at {org}\"],\"bullets\":[{{\"id\":\"e0.b1\",\"text\":\"Scaled Python on Kubernetes at {org}\"}},{{\"id\":\"p1.d\",\"text\":\"Terraform tool\"}}]}}\n```");
        let out = parse_reply(&reply, &v, &r, &sel).unwrap();
        assert!(out.experience[0].bullets.iter().any(|b| b.starts_with("Scaled Python on Kubernetes at ") && !b.contains("[ORG")));
        assert!(out.summary[0].contains("Acme") || out.summary[0].contains("Initech"));
        assert_eq!(out.projects[1].description, "Terraform tool");
        assert_eq!(out.profile, r.profile);
    }

    #[test]
    fn reply_rejections() {
        let (r, jd, _, v) = setup();
        let sel = select(&r, &jd, Budget { max_bullets_per_role: 1, ..Budget::default() });
        assert!(parse_reply(r#"{"summary":["s"],"bullets":[{"id":"e0.b1","text":"x [ORG_9]"}]}"#, &v, &r, &sel).is_err());
        assert!(parse_reply(r#"{"summary":["s"],"bullets":[{"id":"e0.b0","text":"x"}]}"#, &v, &r, &sel).is_err(), "unselected bullet id");
        assert!(parse_reply(r#"{"summary":["s"],"bullets":[{"id":"e9.b9","text":"x"}]}"#, &v, &r, &sel).is_err());
        assert!(parse_reply(r#"{"summary":["s"],"bullets":[]}"#, &v, &r, &sel).is_ok());
        for bad in [r#"{"bullets":[]}"#, r#"{"summary":[],"bullets":[]}"#, r#"{"summary":["  "]}"#] {
            let e = parse_reply(bad, &v, &r, &sel).unwrap_err().to_string();
            assert!(e.contains("summary"), "{e}");
        }
        assert!(parse_reply("no json", &v, &r, &sel).is_err());
    }

    #[test]
    fn fallback_summary_is_grounded() {
        let (mut r, jd) = (fake(), parse_jd(FAKE_JD));
        r.experience[0].date_label = "Jan 2020 - Dec 2023".into();
        let s = fallback_summary(&r, &jd);
        assert!(s.len() >= 2 && s.len() <= 3, "{s:?}");
        assert!(s[0].contains("4+ years") && s[0].contains("Senior Backend Engineer"));
        assert!(s[1].contains("python") && !s[1].contains("graphql"), "only skills the resume mentions: {}", s[1]);
        let all = s.join(" ");
        assert!(!all.contains("40") && all.contains("Built Python services on Kubernetes for Globex Corp"), "{all}");
        assert!(fallback_summary(&Resume::default(), &Jd::default()).len() == 1);
    }

    #[test]
    fn grounded_reply_and_facts_summary() {
        use crate::profile_model::build_facts;
        let (mut r, jd) = (fake(), parse_jd(FAKE_JD));
        r.experience[0].date_label = "Jan 2020 - Dec 2023".into();
        let (sel, v) = (select(&r, &jd, Budget::default()), Vault::from_resume(&r, &[]));
        let facts = build_facts(&r, (2026, 9));
        let reply = r#"{"summary":["Python engineer with 20 years of experience."],"bullets":[{"id":"e0.b1","text":"Built GraphQL services, cutting latency 90%"}]}"#;
        let (out, ev) = parse_reply_grounded(reply, &v, &r, &sel, &facts, &jd).unwrap();
        assert!(out.experience[0].bullets.contains(&r.experience[0].bullets[1]), "reverted");
        assert!(ev.iter().any(|e| e.bullet_id == "e0.b1") && ev.iter().any(|e| e.bullet_id == "summary.0"));
        assert!(out.summary[0].contains("4+ years"), "{:?}", out.summary);
        let s = fallback_summary_with(&facts, &jd, None);
        assert!(s[0].contains("4+ years") && s[1].contains("Python") && !s[1].contains("graphql"), "{s:?}");
        let h = crate::heuristics::Heuristics::default();
        let f = crate::heuristics::Features::from_facts(&facts, &jd, FAKE_JD, None, &h);
        assert_eq!((f.fulltime_role_count, f.internship_count), (2, 0));
        let b = select_with_facts(&r, &jd, Budget::default(), None, &facts);
        assert_eq!(b.bullets.len(), select(&r, &jd, Budget::default()).bullets.len());
    }

    #[test]
    fn fallback_role_and_casing() {
        use crate::profile_model::build_facts;
        let (mut r, jd) = (fake(), parse_jd(&format!("Description\n{FAKE_JD}")));
        r.experience[0].date_label = "Jan 2020 - Dec 2023".into();
        assert_eq!(jd.title.as_deref(), Some("Senior Backend Engineer"), "generic heading is not the title");
        let facts = build_facts(&r, (2026, 9));
        let s = fallback_summary_with(&facts, &jd, None);
        assert!(s[0].contains("targeting Senior Backend Engineer roles") && !s[0].contains("Description"), "{s:?}");
        assert!(s[1].contains("C#") && s[1].contains("Python"), "owner casing, not lowercase: {s:?}");
        assert!(fallback_summary_with(&facts, &jd, Some("Staff Platform Engineer"))[0].contains("Staff Platform Engineer"));
    }

    #[test]
    fn prompt_carries_years_phrase_and_summary_shape() {
        let (mut r, jd) = (fake(), parse_jd(FAKE_JD));
        r.experience[0].date_label = "Jan 2020 - Dec 2023".into();
        let (sel, v) = (select(&r, &jd, Budget::default()), Vault::from_resume(&r, &[]));
        let p = build_payload(&r, &sel, &jd, &v).unwrap();
        let u: serde_json::Value = serde_json::from_str(&p.user).unwrap();
        assert_eq!((u["years_phrase"].as_str(), u["summary_lines"].as_u64()), (Some("4+ years"), Some(3)));
        assert!(p.system.contains("22-32 words") && p.system.contains("**double asterisks**"));
        let none = build_payload(&Resume::default(), &Selection::default(), &jd, &v).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&none.user).unwrap()["years_phrase"].is_null());
    }
}
