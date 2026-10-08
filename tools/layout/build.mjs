import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
const root = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const host = execFileSync("rustc", ["-vV"], {
  encoding: "utf8",
  cwd: root,
}).match(/^host: (.+)$/m)?.[1];
if (!host) throw new Error("Cannot determine Rust host");
const target = process.env.TAURI_ENV_TARGET_TRIPLE || host;
const args = ["build", "--release", "-p", "sampler-layout"];
if (target !== host) args.push("--target", target);
execFileSync("cargo", args, { cwd: root, stdio: "inherit" });
const extension = target.includes("windows") ? ".exe" : "";
const source = path.join(
  root,
  "target",
  ...(target === host ? [] : [target]),
  "release",
  `sampler-layout${extension}`,
);
const binaries = path.join(root, "src-tauri", "binaries");
mkdirSync(binaries, { recursive: true });
copyFileSync(
  source,
  path.join(binaries, `sampler-layout-${target}${extension}`),
);
const debug = path.join(
  root,
  "target",
  ...(target === host ? [] : [target]),
  "debug",
);
mkdirSync(debug, { recursive: true });
copyFileSync(source, path.join(debug, `sampler-layout${extension}`));
