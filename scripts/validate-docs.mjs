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
  "local/docs/decisions/0004-native-provider-control-plane.md",
  "local/checklists/implementation-ready.md",
  "local/checklists/release.md",
  "local/spikes/README.md",
  "local/spikes/s0-1-native-provider-contract.md",
  "local/spikes/s0-2-tauri-core-lifecycle.md",
  "local/spikes/s0-3-terminal-renderer.md",
  "local/spikes/s0-4-generic-pty.md",
  "local/spikes/s0-5-event-store.md",
];

for (const file of publicRequired) {
  if (!fs.existsSync(path.join(repo, file))) errors.push(`missing public file: ${file}`);
}

const gitignore = fs.readFileSync(path.join(repo, ".gitignore"), "utf8");
if (!/^local\/$/m.test(gitignore)) errors.push(".gitignore must contain an exact local/ rule");

const codexConfig = fs.readFileSync(path.join(repo, ".codex/config.toml"), "utf8");
if (/^\[agents\.[^\]]+\]/m.test(codexConfig)) {
  errors.push("custom agents must use standalone .codex/agents TOML files");
}

const reviewerConfig = fs.readFileSync(path.join(repo, ".codex/agents/reviewer.toml"), "utf8");
const reviewerHeader = /^name = "reviewer"\ndescription = "[^"\n]+"\nmodel_reasoning_effort = "xhigh"\nsandbox_mode = "read-only"\ndeveloper_instructions = """\n/;
if (!reviewerHeader.test(reviewerConfig)) {
  errors.push("reviewer config must start with the required standalone reviewer fields");
}
if (!reviewerConfig.trimEnd().endsWith('"""')) {
  errors.push("reviewer config must end with the multiline developer instructions");
}
if (/^\[[^\]]+\]$/m.test(reviewerConfig)) {
  errors.push("reviewer config must not declare TOML sections");
}

const workflow = fs.readFileSync(path.join(repo, "agent-harness/workflow.md"), "utf8");
const reviewerWorkflowMarker =
  "<!-- flit-reviewer-contract:v1 custom=reviewer nested-codex=forbidden fallback=hash-verified -->";
if ((workflow.match(new RegExp(reviewerWorkflowMarker, "g")) ?? []).length !== 1) {
  errors.push("workflow must contain exactly one reviewer contract marker");
}
const nestedCodexRule =
  "- Do not launch a nested Codex client with `codex exec` or another shell command to satisfy this gate.";
if (!workflow.includes(nestedCodexRule) || (workflow.match(/`codex exec`/g) ?? []).length !== 1) {
  errors.push("workflow must contain only the canonical nested Codex prohibition");
}
if (/registered in `.codex\/config\.toml`/i.test(workflow)) {
  errors.push("workflow must not claim reviewer registration in .codex/config.toml");
}

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
  const eventProtocol = fs.readFileSync(path.join(localRoot, "docs/design/event-state-protocol.md"), "utf8");
  const adapterContract = fs.readFileSync(path.join(localRoot, "docs/design/adapter-contract.md"), "utf8");
  const runtimeArchitecture = fs.readFileSync(
    path.join(localRoot, "docs/design/runtime-architecture.md"),
    "utf8",
  );
  const frIds = new Set([...prd.matchAll(/\*\*(FR-\d{3})\b/g)].map((match) => match[1]));
  const nfrIds = new Set([...prd.matchAll(/\*\*(NFR-\d{3})\b/g)].map((match) => match[1]));

  if (frIds.size !== 24) errors.push(`expected 24 local FR definitions, found ${frIds.size}`);
  if (nfrIds.size !== 12) errors.push(`expected 12 local NFR definitions, found ${nfrIds.size}`);
  for (const id of [...frIds, ...nfrIds]) {
    const rows = [...traceability.matchAll(new RegExp(`^\\| ${id} `, `gm`))];
    if (rows.length === 0) errors.push(`missing local traceability row: ${id}`);
    if (rows.length > 1) errors.push(`duplicate local traceability row: ${id}`);
  }

  const tracedIds = new Set(
    [...traceability.matchAll(/^\| ((?:FR|NFR)-\d{3}) /gm)].map((match) => match[1]),
  );
  for (const id of tracedIds) {
    if (!frIds.has(id) && !nfrIds.has(id)) errors.push(`unknown local traceability row: ${id}`);
  }

  for (const eventType of ["question.response_failed", "run.resume_requested", "run.resume_failed"]) {
    if (!eventProtocol.includes(`\`${eventType}\``)) {
      errors.push(`missing required local event contract: ${eventType}`);
    }
  }

  const resumeContractChecks = [
    [adapterContract, "async fn resume("],
    [runtimeArchitecture, "### 7.2 Native Run resume"],
    [runtimeArchitecture, "run.resume_requested"],
    [runtimeArchitecture, "run.resume_failed"],
    [runtimeArchitecture, "session.resumed"],
    [eventProtocol, "| `run.resume_requested` | UI/Core | resume_intent_id,"],
    [eventProtocol, "| `session.resumed` | provider adapter/Core | resume_intent_id,"],
  ];
  for (const [source, phrase] of resumeContractChecks) {
    if (!source.includes(phrase)) errors.push(`incomplete local resume contract: ${phrase}`);
  }

  const requiredTraceabilityPhases = new Map([
    ["FR-009", "4"],
    ["FR-010", "4"],
    ["FR-011", "4"],
  ]);
  for (const [id, requiredPhase] of requiredTraceabilityPhases) {
    const row = traceability.split("\n").find((line) => line.startsWith(`| ${id} `));
    const phaseCell = row?.split("|").at(-2)?.trim() ?? "";
    const phases = new Set(phaseCell.split(",").map((phase) => phase.trim()));
    if (!phases.has(requiredPhase)) {
      errors.push(`${id} traceability must include Phase ${requiredPhase}`);
    }
  }

  const decisionIndex = fs.readFileSync(path.join(localRoot, "docs/decisions/README.md"), "utf8");
  const decisionRows = [
    ...decisionIndex.matchAll(
      /\| (D-\d{3}) \| (Accepted|Provisional|Open|Superseded) \|/g,
    ),
  ];
  const decisionIds = decisionRows.map((match) => match[1]);
  if (new Set(decisionIds).size !== decisionIds.length) errors.push("duplicate local decision ID");
  if (decisionIds.length < 26) {
    errors.push(`expected at least 26 local decisions, found ${decisionIds.length}`);
  }
  for (const [index, id] of decisionIds.entries()) {
    const expected = `D-${String(index + 1).padStart(3, "0")}`;
    if (id !== expected) errors.push(`non-sequential local decision ID: expected ${expected}, found ${id}`);
  }

  const phaseZeroSources = [
    "plan.md",
    "docs/design/verification-strategy.md",
    "docs/delivery/implementation-plan.md",
    "checklists/implementation-ready.md",
    "spikes/README.md",
  ];
  for (const source of phaseZeroSources) {
    const text = fs.readFileSync(path.join(localRoot, source), "utf8");
    for (let spike = 1; spike <= 5; spike += 1) {
      const id = `S0-${spike}`;
      if (!text.includes(id)) errors.push(`missing ${id} in local/${source}`);
    }
  }

  localSummary = `${localMarkdown.length} local markdown files, ${frIds.size} FR, ${nfrIds.size} NFR, ${decisionIds.length} decisions`;
}

if (errors.length > 0) {
  for (const error of errors) console.error(`ERROR ${error}`);
  process.exit(1);
}

console.log(`documentation validation passed: ${publicMarkdown.length} public English markdown files; ${localSummary}`);
