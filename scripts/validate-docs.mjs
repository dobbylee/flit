import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const localRoot = path.join(repo, "local");
const errors = [];

const publicRequired = [
  "README.md",
  "AGENTS.md",
  "LICENSE",
  ".gitignore",
  ".codex/config.toml",
  ".codex/agents/reviewer.toml",
  "agent-harness/workflow.md",
  "agent-harness/prompts/implementation-review.md",
  "agent-harness/templates/task-plan.md",
  "scripts/validate-docs.sh",
  "scripts/validate-docs.mjs",
];

const localRequired = [
  "local/README.md",
  "local/plan.md",
  "local/docs/product/prd.md",
  "local/docs/design/ux-spec.md",
  "local/docs/design/event-state-protocol.md",
  "local/docs/design/domain-data.md",
  "local/docs/design/runtime-architecture.md",
  "local/docs/design/adapter-contract.md",
  "local/docs/design/security-reliability.md",
  "local/docs/design/verification-strategy.md",
  "local/docs/delivery/implementation-plan.md",
  "local/docs/delivery/traceability.md",
  "local/docs/decisions/README.md",
  "local/checklists/implementation-ready.md",
  "local/checklists/release.md",
];

for (const file of publicRequired) {
  if (!fs.existsSync(path.join(repo, file))) errors.push(`missing public file: ${file}`);
}

const gitignore = fs.readFileSync(path.join(repo, ".gitignore"), "utf8");
if (!/^local\/$/m.test(gitignore)) errors.push(".gitignore must contain an exact local/ rule");

function collectFiles(directory, predicate, skippedNames = new Set()) {
  const results = [];
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    if (skippedNames.has(entry.name)) continue;
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) results.push(...collectFiles(full, predicate, skippedNames));
    if (entry.isFile() && predicate(full)) results.push(full);
  }
  return results;
}

const publicMarkdown = collectFiles(
  repo,
  (file) => file.endsWith(".md"),
  new Set([".git", "local", "node_modules", "target", "dist"]),
);

const publicLanguageFiles = [
  ...publicMarkdown,
  ...collectFiles(path.join(repo, ".codex"), (file) => file.endsWith(".toml")),
];

const hangul = /[\u3131-\u318E\uAC00-\uD7A3]/;
for (const file of publicLanguageFiles) {
  const text = fs.readFileSync(file, "utf8");
  if (hangul.test(text)) errors.push(`public documentation must be English-only: ${path.relative(repo, file)}`);
}

const unfinished = /\b(?:TODO|TBD|FIXME):/g;
const linkPattern = /\[[^\]]*\]\(([^)]+)\)/g;

function validateMarkdown(files, { publicScope = false } = {}) {
  for (const file of files) {
    const text = fs.readFileSync(file, "utf8");
    const relative = path.relative(repo, file);

    for (const match of text.matchAll(unfinished)) {
      const line = text.slice(0, match.index).split("\n").length;
      errors.push(`unfinished marker: ${relative}:${line}`);
    }

    for (const match of text.matchAll(linkPattern)) {
      let target = match[1].trim();
      if (target.startsWith("<") && target.endsWith(">")) target = target.slice(1, -1);
      if (/^(?:https?:|mailto:|#)/.test(target)) continue;
      target = target.split("#", 1)[0];
      if (!target) continue;

      const resolved = path.resolve(path.dirname(file), decodeURIComponent(target));
      const line = text.slice(0, match.index).split("\n").length;
      if (!fs.existsSync(resolved)) {
        errors.push(`broken local link: ${relative}:${line} -> ${match[1]}`);
      }
      if (publicScope && (resolved === localRoot || resolved.startsWith(`${localRoot}${path.sep}`))) {
        errors.push(`public documentation must not link to ignored local content: ${relative}:${line}`);
      }
    }
  }
}

validateMarkdown(publicMarkdown, { publicScope: true });

let localSummary = "local design tree not present";
if (fs.existsSync(localRoot)) {
  for (const file of localRequired) {
    if (!fs.existsSync(path.join(repo, file))) errors.push(`missing local planning file: ${file}`);
  }

  const localMarkdown = collectFiles(localRoot, (file) => file.endsWith(".md"));
  validateMarkdown(localMarkdown);

  const prd = fs.readFileSync(path.join(localRoot, "docs/product/prd.md"), "utf8");
  const traceability = fs.readFileSync(path.join(localRoot, "docs/delivery/traceability.md"), "utf8");
  const frIds = new Set([...prd.matchAll(/\*\*(FR-\d{3})\b/g)].map((match) => match[1]));
  const nfrIds = new Set([...prd.matchAll(/\*\*(NFR-\d{3})\b/g)].map((match) => match[1]));

  if (frIds.size !== 24) errors.push(`expected 24 local FR definitions, found ${frIds.size}`);
  if (nfrIds.size !== 12) errors.push(`expected 12 local NFR definitions, found ${nfrIds.size}`);
  for (const id of [...frIds, ...nfrIds]) {
    if (!traceability.includes(`| ${id} `)) errors.push(`missing local traceability row: ${id}`);
  }

  const decisionIndex = fs.readFileSync(path.join(localRoot, "docs/decisions/README.md"), "utf8");
  const decisionIds = [...decisionIndex.matchAll(/\| (D-\d{3}) \|/g)].map((match) => match[1]);
  if (new Set(decisionIds).size !== decisionIds.length) errors.push("duplicate local decision ID");
  if (decisionIds.length < 19) errors.push(`expected at least 19 local decisions, found ${decisionIds.length}`);

  localSummary = `${localMarkdown.length} local markdown files, ${frIds.size} FR, ${nfrIds.size} NFR, ${decisionIds.length} decisions`;
}

if (errors.length > 0) {
  for (const error of errors) console.error(`ERROR ${error}`);
  process.exit(1);
}

console.log(`documentation validation passed: ${publicMarkdown.length} public English markdown files; ${localSummary}`);
