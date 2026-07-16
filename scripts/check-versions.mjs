import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = dirname(dirname(fileURLToPath(import.meta.url)));

function readJson(relativePath) {
  try {
    return JSON.parse(readText(relativePath));
  } catch (error) {
    throw new Error(`无法读取 ${relativePath}：${error.message}`);
  }
}

function readText(relativePath) {
  return readFileSync(join(projectRoot, relativePath), "utf8");
}

function cargoPackageVersion(relativePath, packageName) {
  const blocks = readText(relativePath).split(/^\[\[?package\]?\]\s*$/m).slice(1);
  const matches = blocks.filter((block) => {
    const name = block.match(/^name\s*=\s*"([^"]+)"\s*$/m)?.[1];
    return packageName == null || name === packageName;
  });
  if (matches.length !== 1) {
    throw new Error(`${relativePath} 中应有且仅有一个目标 package 区段`);
  }
  return requireVersion(matches[0].match(/^version\s*=\s*"([^"]+)"\s*$/m)?.[1], relativePath);
}

function requireVersion(value, source) {
  if (typeof value !== "string" || !/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(value)) {
    throw new Error(`${source} 缺少有效的语义版本号`);
  }
  return value;
}

const packageJson = readJson("package.json");
const packageLock = readJson("package-lock.json");
const tauriConfig = readJson("src-tauri/tauri.conf.json");

const versions = new Map([
  ["package.json", requireVersion(packageJson.version, "package.json")],
  ["package-lock.json", requireVersion(packageLock.version, "package-lock.json")],
  ["package-lock.json packages 根节点", requireVersion(packageLock.packages?.[""]?.version, "package-lock.json packages 根节点")],
  ["src-tauri/tauri.conf.json", requireVersion(tauriConfig.version, "src-tauri/tauri.conf.json")],
  ["src-tauri/Cargo.toml", cargoPackageVersion("src-tauri/Cargo.toml")],
  ["src-tauri/Cargo.lock", cargoPackageVersion("src-tauri/Cargo.lock", packageJson.name)],
]);
const expectedVersion = versions.get("package.json");
for (const [source, version] of versions) {
  if (version !== expectedVersion) {
    throw new Error(`版本不一致：${source} 为 ${version}，期望 ${expectedVersion}`);
  }
}

const tagIndex = process.argv.indexOf("--tag");
if (tagIndex >= 0) {
  const tag = process.argv[tagIndex + 1];
  if (!tag) throw new Error("--tag 缺少参数");
  if (tag !== `v${expectedVersion}`) {
    throw new Error(`发布标签 ${tag} 与应用版本 v${expectedVersion} 不一致`);
  }
}

console.log(`版本检查通过：${expectedVersion}`);
