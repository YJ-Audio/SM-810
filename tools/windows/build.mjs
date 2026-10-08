import { execFileSync } from "node:child_process";
import { copyFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

if (process.platform !== "win32") {
  throw new Error(
    "Build the Windows installer on Windows with the MSVC toolchain.",
  );
}
const root = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const cli = path.join(root, "ui/node_modules/@tauri-apps/cli/tauri.js");
const tauri = (args) =>
  execFileSync(process.execPath, [cli, ...args], {
    cwd: root,
    stdio: "inherit",
  });
// Native dependencies emit their runtime DLLs during compilation, before bundling.
tauri(["build", "--no-bundle"]);
const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
    cwd: root,
    encoding: "utf8",
  }),
);
const release = path.join(metadata.target_directory, "release");
// Keep the installer per-user: deploy the redistributable CRT beside the application.
let redist = process.env.VCToolsRedistDir;
if (!redist) {
  const vswhere = path.join(
    process.env["ProgramFiles(x86)"],
    "Microsoft Visual Studio/Installer/vswhere.exe",
  );
  const installation = execFileSync(
    vswhere,
    [
      "-latest",
      "-products",
      "*",
      "-requires",
      "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
      "-property",
      "installationPath",
    ],
    { encoding: "utf8" },
  ).trim();
  const base = path.join(installation, "VC/Redist/MSVC");
  const version = readdirSync(base)
    .filter((name) => /^\d+\./.test(name))
    .sort((a, b) => b.localeCompare(a, undefined, { numeric: true }))[0];
  if (!version)
    throw new Error("Visual C++ redistributable directory was not found.");
  redist = path.join(base, version);
}
const architecture = path.join(redist, "x64");
const directory = readdirSync(architecture).find((name) =>
  /^Microsoft\.VC\d+\.CRT$/.test(name),
);
if (!directory)
  throw new Error("The x64 Visual C++ redistributable CRT was not found.");
const crt = path.join(architecture, directory);
const runtimeDlls = readdirSync(crt).filter((name) => name.endsWith(".dll"));
for (const name of runtimeDlls)
  copyFileSync(path.join(crt, name), path.join(release, name));
const dlls = readdirSync(release).filter(
  (name) =>
    /^(DirectML|onnxruntime(?:_providers_.+)?)\.dll$/i.test(name) ||
    runtimeDlls.includes(name),
);
if (!dlls.some((name) => name.toLowerCase() === "directml.dll")) {
  throw new Error(
    "DirectML.dll is missing from the build output; the installer cannot be packaged.",
  );
}
const resources = Object.fromEntries(
  dlls.map((name) => [path.join(release, name), name]),
);
tauri([
  "bundle",
  "--bundles",
  "nsis",
  "--config",
  JSON.stringify({ bundle: { resources } }),
]);
