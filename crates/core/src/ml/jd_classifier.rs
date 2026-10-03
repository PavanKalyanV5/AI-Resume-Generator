//! Role-family and seniority classifier for JDs: naive Bayes seeded with synthetic documents
//! built from the editable keyword lists below, then corrected online by the user.
use super::core::*;
use serde::{Deserialize, Serialize};

pub const FAMILIES: [&str; 11] = ["backend", "frontend", "fullstack", "data_eng", "ml_ai", "devops_cloud", "mobile", "security", "embedded", "qa", "management"];
pub const SENIORITIES: [&str; 4] = ["junior", "mid", "senior", "lead"];

/// EDITABLE weak-label vocabulary: lowercase single tokens (jd::tokens splits on non-alphanumerics).
pub const FAMILY_KEYWORDS: [(&str, &[&str]); 11] = [
    ("backend", &["backend", "api", "apis", "rest", "microservices", "java", "spring", "django", "flask", "node", "golang", "postgresql", "mysql", "redis", "kafka", "server", "databases", "scalable", "grpc", "sql", "python", "services"]),
    ("frontend", &["frontend", "react", "angular", "vue", "typescript", "javascript", "css", "html", "ui", "ux", "responsive", "browser", "nextjs", "webpack", "accessibility", "components", "tailwind", "redux", "design", "figma"]),
    ("fullstack", &["fullstack", "full", "stack", "end", "react", "node", "typescript", "api", "frontend", "backend", "postgresql", "web", "mern", "django", "features", "ui", "rest"]),
    ("data_eng", &["data", "pipelines", "etl", "spark", "airflow", "warehouse", "snowflake", "dbt", "hadoop", "bigquery", "kafka", "sql", "python", "lakehouse", "ingestion", "orchestration", "batch", "streaming", "databricks"]),
    ("ml_ai", &["machine", "learning", "ml", "ai", "models", "pytorch", "tensorflow", "nlp", "llm", "deep", "neural", "training", "embeddings", "mlops", "inference", "transformers", "python", "scikit", "research", "computer", "vision"]),
    ("devops_cloud", &["devops", "kubernetes", "docker", "terraform", "aws", "azure", "gcp", "cicd", "jenkins", "sre", "infrastructure", "cloud", "monitoring", "ansible", "helm", "prometheus", "reliability", "deployment", "linux", "pipelines"]),
    ("mobile", &["mobile", "ios", "android", "swift", "kotlin", "flutter", "react", "native", "swiftui", "xcode", "appstore", "app", "apps", "jetpack", "dart", "push", "notifications"]),
    ("security", &["security", "pentest", "vulnerability", "siem", "threat", "soc", "encryption", "compliance", "iam", "owasp", "incident", "firewall", "cryptography", "malware", "appsec", "audit", "risk"]),
    ("embedded", &["embedded", "firmware", "c", "rtos", "microcontroller", "arm", "fpga", "iot", "sensors", "drivers", "hardware", "bootloader", "uart", "spi", "i2c", "realtime", "bsp", "yocto"]),
    ("qa", &["qa", "quality", "testing", "test", "automation", "selenium", "cypress", "playwright", "regression", "manual", "sdet", "bugs", "testcases", "coverage", "defects", "junit", "pytest"]),
    ("management", &["manager", "management", "hiring", "stakeholders", "roadmap", "budget", "people", "reports", "performance", "reviews", "strategy", "leadership", "director", "organization", "coaching", "headcount", "okrs", "mentoring"]),
];
/// EDITABLE seniority cues (years tokens included on purpose: "5+ years" -> "5", "years").
pub const SENIORITY_KEYWORDS: [(&str, &[&str]); 4] = [
    ("junior", &["junior", "entry", "graduate", "intern", "trainee", "fresher", "0", "1", "2", "learn", "assist", "supervision", "associate"]),
    ("mid", &["mid", "intermediate", "independently", "3", "4", "midlevel", "ownership", "contribute", "collaborate"]),
    ("senior", &["senior", "sr", "5", "6", "7", "8", "expert", "deep", "architect", "mentor", "complex", "proven", "extensive"]),
    ("lead", &["lead", "principal", "staff", "head", "10", "12", "manager", "direct", "organization", "vision", "strategy", "leadership", "teams"]),
];
const FILLER: [&str; 12] = ["we", "are", "looking", "for", "experience", "years", "team", "work", "with", "strong", "skills", "build"];
const DOCS_PER_CLASS: usize = 30;
const CORRECTION_WEIGHT: f32 = 3.0;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    pub family: Vec<(String, f32)>,
    pub seniority: Vec<(String, f32)>,
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JdClassifier {
    pub family: NaiveBayes,
    pub seniority: NaiveBayes,
    /// Calls to `correct` and how often the pre-update top-1 already matched (prequential accuracy).
    pub corrections: usize,
    pub family_hits: usize,
    pub seniority_hits: usize,
    pub updated_at: String,
}
impl Default for JdClassifier {
    fn default() -> Self {
        Self::seed()
    }
}

fn synth(nb: &mut NaiveBayes, table: &[(&str, &[&str])], seed: u64) {
    let mut rng = Rng::new(seed);
    for (label, kws) in table {
        for _ in 0..DOCS_PER_CLASS {
            let mut doc: Vec<String> = (0..10).map(|_| kws[(rng.next_u64() % kws.len() as u64) as usize].to_string()).collect();
            doc.extend((0..6).map(|_| FILLER[(rng.next_u64() % FILLER.len() as u64) as usize].to_string()));
            nb.update(&doc, label, 1.0);
        }
    }
}

impl JdClassifier {
    /// Cold-start model trained only on synthetic keyword documents (deterministic).
    pub fn seed() -> Self {
        let (mut family, mut seniority) = (NaiveBayes::new(&FAMILIES), NaiveBayes::new(&SENIORITIES));
        synth(&mut family, &FAMILY_KEYWORDS, 101);
        synth(&mut seniority, &SENIORITY_KEYWORDS, 202);
        JdClassifier { family, seniority, corrections: 0, family_hits: 0, seniority_hits: 0, updated_at: String::new() }
    }
    pub fn classify(&self, text: &str) -> Classification {
        let t = crate::jd::tokens(text);
        let sort = |mut v: Vec<(String, f32)>| {
            v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            v
        };
        let family = sort(self.family.predict(&t));
        Classification { evidence: self.family.evidence(&t, 6), family, seniority: sort(self.seniority.predict(&t)) }
    }
    /// User confirms/corrects labels for a JD (call on confirmations too, it keeps the online
    /// accuracy honest). Unknown labels are ignored.
    pub fn correct(&mut self, jd_text: &str, family: &str, seniority: &str) {
        let before = self.classify(jd_text);
        self.corrections += 1;
        self.family_hits += (before.family[0].0 == family) as usize;
        self.seniority_hits += (before.seniority[0].0 == seniority) as usize;
        let t = crate::jd::tokens(jd_text);
        self.family.update(&t, family, CORRECTION_WEIGHT);
        self.seniority.update(&t, seniority, CORRECTION_WEIGHT);
        self.updated_at = now_iso();
    }
    pub fn cards(&self) -> Vec<ModelCard> {
        let n = self.corrections;
        let mk = |name: &str, hits: usize, base: f32| {
            let mut c = ModelCard::new(name, n, "online top-1 accuracy", (n > 0).then(|| hits as f32 / n as f32), Some(base), true, &self.updated_at, "Seeded from keyword lists; n counts user confirmations/corrections; accuracy is measured before each update.");
            c.learning_curve = vec![];
            c
        };
        vec![mk("jd_family", self.family_hits, 1.0 / FAMILIES.len() as f32), mk("jd_seniority", self.seniority_hits, 1.0 / SENIORITIES.len() as f32)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(c: &Classification) -> &str {
        &c.family[0].0
    }

    #[test]
    fn seeded_families_and_online_correction() {
        let mut m = JdClassifier::seed();
        let backend = "Backend engineer to build scalable REST APIs and microservices in Java and Spring with PostgreSQL, Redis and Kafka. Own server-side services.";
        let ml = "Machine learning engineer: train deep neural models in PyTorch, work on NLP and LLM embeddings, MLOps and inference for transformers.";
        let devops = "DevOps engineer for Kubernetes, Docker, Terraform on AWS; CI/CD, monitoring with Prometheus, SRE reliability and infrastructure automation.";
        let front = "Frontend developer with React, TypeScript, CSS and HTML; responsive UI components, accessibility and Tailwind.";
        assert_eq!(top(&m.classify(backend)), "backend");
        assert_eq!(top(&m.classify(ml)), "ml_ai");
        assert_eq!(top(&m.classify(devops)), "devops_cloud");
        assert_eq!(top(&m.classify(front)), "frontend");
        assert!(!m.classify(ml).evidence.is_empty());
        let s = m.classify("Senior engineer, 8+ years, mentor others, deep expertise in complex systems");
        assert_eq!(s.seniority[0].0, "senior");
        let j = m.classify("Junior developer, entry level graduate, will learn under supervision");
        assert_eq!(j.seniority[0].0, "junior");

        let odd = "Solidity smart contracts on Ethereum with blockchain auditing";
        let before = m.classify(odd).family.iter().find(|x| x.0 == "backend").unwrap().1;
        m.correct(odd, "backend", "mid");
        m.correct(odd, "backend", "mid");
        let after = m.classify(odd);
        assert_eq!(top(&after), "backend");
        assert!(after.family[0].1 > before);
        assert_eq!(m.cards()[0].n_samples, 2);
        let rt: JdClassifier = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(rt, m);
    }
}
