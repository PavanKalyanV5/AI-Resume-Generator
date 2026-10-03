// One-off: artifacts/**/perosnal-data/*.ts (+ phone/client from the .tex) -> data/resume.json.
// Run: node --experimental-strip-types scripts/export-ts.mjs
// Output holds PII and lives in gitignored data/. Nothing is printed except counts.
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";

const root = resolve("artifacts/resume-latext-template");
const d = (f) => pathToFileURL(`${root}/perosnal-data/${f}.ts`).href;
const [{ profile }, { socials }, { experience }, { education }, { skillCategories }, { projects }, { certifications }] =
  await Promise.all(["profile", "socials", "experience", "education", "skills", "projects", "certifications"].map((f) => import(d(f))));

const tex = (f) => readFileSync(`${root}/sections/${f}.tex`, "utf8");

// phone: from header.tex `\href{tel:...}{display}`
const phone = tex("header").match(/\\href\{tel:[^}]*\}\{([^}]+)\}/)?.[1] ?? null;

// client: from experience.tex `{Org}{Client: X}{Location}`; map by order to Software Engineer roles
const clients = [...tex("experience").matchAll(/\{Client:\s*([^}]+)\}/g)].map((m) => m[1].trim());
const tailored = experience.map((e) => ({ ...e }));
const withClient = tailored.filter((e) => /^software engineer/i.test(e.role));
withClient.forEach((e, i) => (e.client = clients[i] ?? clients[0]));

const resume = {
  profile: { ...profile, phone },
  socials,
  summary: [],
  experience: tailored,
  education,
  skills: skillCategories,
  projects,
  certifications,
};
mkdirSync("data", { recursive: true });
writeFileSync("data/resume.json", JSON.stringify(resume, null, 2));
console.log(
  `exported: ${tailored.length} roles (${withClient.length} with client), ${projects.length} projects, ${certifications.length} certs, phone=${phone ? "yes" : "no"}`,
);
