import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const name = "sudo-mudo-apt-opt-gop-stop-remove";
const scope = "@archhaze24";
const tag = "v0.0.0";
const platforms = [
  ["linux-x64", "x86_64-unknown-linux-gnu", "linux", "x64", "glibc"],
  ["linux-x64-musl", "x86_64-unknown-linux-musl", "linux", "x64", "musl"],
  ["linux-arm64", "aarch64-unknown-linux-gnu", "linux", "arm64", "glibc"],
  ["linux-arm64-musl", "aarch64-unknown-linux-musl", "linux", "arm64", "musl"],
  ["darwin-x64", "x86_64-apple-darwin", "darwin", "x64"],
  ["darwin-arm64", "aarch64-apple-darwin", "darwin", "arm64"],
  ["win32-x64", "x86_64-pc-windows-msvc", "win32", "x64"],
];
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "npm-packaging-test-"));
const assets = path.join(tmp, "assets");
const out = path.join(tmp, "dist");
const fake = '#!/usr/bin/env node\nprocess.stdout.write(JSON.stringify(process.argv.slice(2)));\nprocess.exit(37);\n';
after(() => fs.rmSync(tmp, { recursive: true, force: true }));

function runBuild(output = out, input = assets, releaseTag = tag) {
  const result = spawnSync(process.execPath, [path.join(root, "npm/build.mjs"), releaseTag, "--assets", input, "--out", output], { encoding: "utf8", timeout: 30000, killSignal: "SIGKILL" });
  assert.ifError(result.error);
  return result;
}

function json(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

before(() => {
  fs.mkdirSync(assets);
  const source = path.join(tmp, "source");
  fs.mkdirSync(source);
  for (const [, target, platform] of platforms) {
    const windows = platform === "win32";
    const executable = `${name}${windows ? ".exe" : ""}`;
    fs.writeFileSync(path.join(source, executable), fake);
    const archive = path.join(assets, `${name}-${tag}-${target}.${windows ? "zip" : "tar.gz"}`);
    if (windows) execFileSync("python3", ["-m", "zipfile", "-c", archive, executable], { cwd: source, timeout: 30000 });
    else execFileSync("tar", ["-czf", archive, executable], { cwd: source, timeout: 30000 });
    fs.writeFileSync(`${archive}.sha256`, `${createHash("sha256").update(fs.readFileSync(archive)).digest("hex")}  ${path.basename(archive)}\n`);
    fs.unlinkSync(path.join(source, executable));
  }

});

test("builds packages from verified local release archives", () => {
  const result = runBuild();
  assert.equal(result.status, 0, result.stderr);
  const dependencies = {};
  for (const [suffix, , platform, cpu, libc] of platforms) {
    const packageName = `${scope}/${name}-${suffix}`;
    const dir = path.join(out, `${name}-${suffix}`);
    const manifest = json(path.join(dir, "package.json"));
    assert.equal(manifest.name, packageName);
    assert.equal(manifest.version, "0.0.0");
    assert.deepEqual(manifest.os, [platform]);
    assert.deepEqual(manifest.cpu, [cpu]);
    assert.deepEqual(manifest.libc, libc ? [libc] : undefined);
    assert.equal(fs.readFileSync(path.join(dir, "bin", `${name}${platform === "win32" ? ".exe" : ""}`), "utf8"), fake);
    assert.deepEqual(fs.readFileSync(path.join(dir, "LICENSE")), fs.readFileSync(path.join(root, "LICENSE")));
    dependencies[packageName] = "0.0.0";
  }
  const launcher = path.join(out, name);
  assert.deepEqual(json(path.join(launcher, "package.json")).optionalDependencies, dependencies);
  assert.deepEqual(fs.readFileSync(path.join(launcher, "LICENSE")), fs.readFileSync(path.join(root, "LICENSE")));
});

test("launcher forwards arguments and exit status", { skip: process.platform !== "linux" || !["x64", "arm64"].includes(process.arch) }, () => {
  const launcherOut = path.join(tmp, "launcher-dist");
  const built = runBuild(launcherOut);
  assert.equal(built.status, 0, built.stderr);
  const modules = path.join(tmp, "node_modules", scope);
  fs.mkdirSync(modules, { recursive: true });
  fs.cpSync(path.join(launcherOut, name), path.join(modules, name), { recursive: true });
  // provide both variants so the test works on glibc and musl hosts.
  for (const suffix of [`linux-${process.arch}`, `linux-${process.arch}-musl`]) {
    fs.cpSync(path.join(launcherOut, `${name}-${suffix}`), path.join(modules, `${name}-${suffix}`), { recursive: true });
  }
  const args = ["space in argument", "--example", "", "quoted'argument"];
  const result = spawnSync(process.execPath, [path.join(modules, name, "bin", `${name}.js`), ...args], { encoding: "utf8", timeout: 30000, killSignal: "SIGKILL" });
  assert.ifError(result.error);
  assert.equal(result.status, 37, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), args);
});

test("refuses an unmarked nonempty output directory", () => {
  const guarded = path.join(tmp, "guarded");
  fs.mkdirSync(guarded);
  const sentinel = path.join(guarded, "sentinel");
  fs.writeFileSync(sentinel, "keep");
  const result = runBuild(guarded);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /refusing to overwrite non-empty/);
  assert.equal(fs.readFileSync(sentinel, "utf8"), "keep");
});

test("rejects a checksum mismatch", () => {
  const invalid = path.join(tmp, "invalid-assets");
  fs.cpSync(assets, invalid, { recursive: true });
  const archive = path.join(invalid, `${name}-${tag}-${platforms[0][1]}.tar.gz`);
  fs.writeFileSync(`${archive}.sha256`, `${"0".repeat(64)}  ${path.basename(archive)}\n`);
  const result = runBuild(path.join(tmp, "bad-checksum"), invalid);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /checksum mismatch/);
});

test("rejects malformed tags before clearing a previous build", () => {
  const guarded = path.join(tmp, "previous-build");
  fs.mkdirSync(guarded);
  fs.writeFileSync(path.join(guarded, `.${name}-npm-dist`), "previous build");
  const sentinel = path.join(guarded, "sentinel");
  fs.writeFileSync(sentinel, "keep");
  for (const invalid of ["v0.0.0/other", "v0.0.0suffix", "v0.0.0 ", "v0.0.0\n"]) {
    const result = runBuild(guarded, assets, invalid);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /unexpected argument/);
    assert.equal(fs.readFileSync(sentinel, "utf8"), "keep");
  }
});

test("accepts prerelease tags with build metadata", () => {
  const releaseTag = "v0.0.0-rc.1+build.2";
  const input = path.join(tmp, "prerelease-assets");
  fs.mkdirSync(input);
  for (const file of fs.readdirSync(assets)) {
    fs.copyFileSync(path.join(assets, file), path.join(input, file.replace(tag, releaseTag)));
  }
  const output = path.join(tmp, "prerelease-dist");
  const result = runBuild(output, input, releaseTag);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(json(path.join(output, name, "package.json")).version, releaseTag.slice(1));
});
