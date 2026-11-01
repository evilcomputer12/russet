// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// W19 smoke run in the installed Russet Development editor (Windows), with this
// worktree's extension as the development copy, its own --user-data-dir and
// --extensions-dir under extension/.vscode-test/, and devices "none".
//
//   node test/smoke/run-installed.mjs --screens   synthetic daemon + fake setup runner;
//                                                 drives every W19 surface and saves screenshots
//   node test/smoke/run-installed.mjs --live      the managed backend's daemon (devices none) and
//                                                 the real setup runner's `--setup status` only
//
// Screenshots need playwright-core (PLAYWRIGHT_CORE, default: the Codex runtime's copy);
// the editor is started with --remote-debugging-port and driven over CDP.

/* global setTimeout */
import { runTests } from "@vscode/test-electron";
import { spawn, spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import * as fs from "node:fs";
import { createRequire } from "node:module";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { startFakeDaemon } from "./fake-daemon.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const ext = path.resolve(here, "../..");
const live = process.argv.includes("--live");
const mode = live ? "live" : "screens";
const installation = path.join(process.env.LOCALAPPDATA, "Programs", "Russet Development");
const editorExe = path.join(installation, "editor", "Russet.exe");
const runtime = path.join(ext, ".vscode-test", `w19-${mode}`);
const shots = path.join(runtime, "shots");
const bridge = path.join(runtime, "bridge");
const port = Number(process.env.W19_CDP_PORT || (live ? 9341 : 9339));
const playwrightPath = process.env.PLAYWRIGHT_CORE ||
  path.join(process.env.USERPROFILE, ".cache", "codex-runtimes", "codex-primary-runtime", "dependencies", "node", "node_modules", "playwright-core");
const liveRunner = process.env.W19_LIVE_RUNNER ||
  path.resolve(ext, "..", "..", "russet-windows", "src", "Russet.Host", "bin", "Release", "net10.0", "Russet.Host.exe");

fs.rmSync(runtime, { recursive: true, force: true });
for (const d of [shots, bridge, path.join(runtime, "user", "User"), path.join(runtime, "extensions")]) {
  fs.mkdirSync(d, { recursive: true });
}

const settings = {
  "workbench.colorTheme": "Default Dark Modern",
  "workbench.startupEditor": "none",
  "workbench.tips.enabled": false,
  "window.restoreWindows": "none",
  "window.newWindowDimensions": "default",
  "update.mode": "none",
  "extensions.autoCheckUpdates": false,
  "extensions.autoUpdate": false,
  "security.workspace.trust.enabled": false,
  "telemetry.telemetryLevel": "off",
  "russet.run.openConsole": false,
  "git.openRepositoryInParentFolders": "never",
};
const env = { SMOKE_DIR: bridge };
const cleanups = [];
const results = { mode, checks: [], screenshots: [] };

if (live) {
  // As russet-windows/tests/installed-editor.mjs: a throwaway engine home, devices "none".
  const home = `/home/russet/.russet-dev/w19-smoke-${randomUUID()}`;
  const wsl = path.join(process.env.SystemRoot, "System32", "wsl.exe");
  const engineEnv = ["env", `RUSSET_HOME=${home}`, "RUSSET_DEVICE_BACKEND=none", "RUSSET_SECRETS_BACKEND=memory"];
  const daemon = spawn(wsl, ["-d", "Russet-Dev", "-u", "russet", "--exec", ...engineEnv,
    "/opt/russet/bin/russet-recovered", "daemon", "run", "--foreground", "--idle-timeout", "120s"], { windowsHide: true, stdio: "ignore" });
  cleanups.push(() => {
    spawnSync(wsl, ["-d", "Russet-Dev", "-u", "russet", "--exec", ...engineEnv, "/opt/russet/bin/russet-recovered", "daemon", "stop"], { windowsHide: true, stdio: "ignore" });
    if (daemon.exitCode === null) daemon.kill();
    spawnSync(wsl, ["-d", "Russet-Dev", "-u", "russet", "--exec", "rm", "-rf", home], { windowsHide: true, stdio: "ignore" });
  });
  Object.assign(env, {
    RUSSET_WINDOWS_HOST: path.join(installation, "host", "Russet.Host.exe"),
    RUSSET_BACKEND_HOME: home,
    RUSSET_SETUP_RUNNER: liveRunner,
  });
} else {
  delete process.env.RUSSET_WINDOWS_HOST;
  const pipe = `\\\\.\\pipe\\russet-w19-smoke-${randomUUID().slice(0, 8)}`;
  const server = await startFakeDaemon(pipe, path.join(ext, "test", "fixtures", "HelloProject"));
  cleanups.push(() => server.close());
  settings["russet.daemon.socketPath"] = pipe;
  settings["russet.daemon.autoStart"] = false;
  const fakeHome = path.join(runtime, "home");
  fs.mkdirSync(path.join(fakeHome, "Downloads"), { recursive: true });
  fs.writeFileSync(path.join(fakeHome, "Downloads", "Xcode_27.2.xip"), "synthetic archive for the W19 screenshot pass\n");
  Object.assign(env, {
    RUSSET_SETUP_RUNNER: path.join(ext, "test", "fixtures", "fake-setup-runner.mjs"),
    FAKE_SETUP_STATE: path.join(runtime, "setup-state.json"),
    FAKE_SETUP_DELAY: "220",
    USERPROFILE: fakeHome,
  });
}
fs.writeFileSync(path.join(runtime, "user", "User", "settings.json"), JSON.stringify(settings, null, 2));

// --- The command bridge to suite.cjs -----------------------------------------------

let seq = 0;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function waitFor(fn, what, timeout = 30000) {
  const end = Date.now() + timeout;
  let last;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (err) {
      last = err;
    }
    await sleep(150);
  }
  throw new Error(`Timed out waiting for ${what}${last ? `: ${last.message}` : ""}`);
}
async function command(name, args = [], opts = {}) {
  seq++;
  fs.writeFileSync(path.join(bridge, `cmd-${seq}.json`), JSON.stringify({ command: name, args, ...opts }));
  const ackFile = path.join(bridge, `ack-${seq}.json`);
  const ack = await waitFor(() => fs.existsSync(ackFile) && JSON.parse(fs.readFileSync(ackFile, "utf8")), `ack of ${name}`, 60000);
  if (!ack.ok) throw new Error(`${name} failed: ${ack.error}`);
  return ack.result;
}
function check(name, ok, detail = "") {
  results.checks.push({ name, ok: !!ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail ? ` — ${detail}` : ""}`);
}

// --- CDP ------------------------------------------------------------------------------

let page;
async function connect() {
  if (!fs.existsSync(playwrightPath)) {
    console.log(`playwright-core not found at ${playwrightPath}; screenshots skipped.`);
    return false;
  }
  const require = createRequire(import.meta.url);
  const { chromium } = require(playwrightPath);
  const browser = await waitFor(() => chromium.connectOverCDP(`http://127.0.0.1:${port}`), "the editor's debug port", 60000);
  page = await waitFor(() => browser.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes("workbench")), "the workbench page");
  try {
    const cdp = await browser.newBrowserCDPSession();
    const { windowId } = await cdp.send("Browser.getWindowForTarget", {});
    await cdp.send("Browser.setWindowBounds", { windowId, bounds: { windowState: "normal" } });
    await cdp.send("Browser.setWindowBounds", { windowId, bounds: { left: 40, top: 20, width: 1440, height: 920 } });
  } catch (err) {
    console.log(`window resize skipped: ${err.message}`);
  }
  return true;
}
async function webview(title, timeout = 20000) {
  return waitFor(async () => {
    if (process.env.W19_DEBUG_FRAMES) {
      console.log((await Promise.all(page.frames().map(async (f) => `${f.url().slice(0, 90)} | ${await f.title().catch(() => "?")}`))).join(" ;; "));
    }
    for (const f of page.frames()) {
      if (f.url().startsWith("vscode-webview://") && (await f.title().catch(() => "")) === title) {
        if (await f.locator("#root").count()) return f;
      }
    }
    return undefined;
  }, `the "${title}" webview`, timeout);
}
async function shot(name, note) {
  if (!page) return;
  await sleep(400);
  const file = path.join(shots, `${name}.png`);
  await page.screenshot({ path: file });
  results.screenshots.push({ file, note });
  console.log(`screenshot ${file}`);
}

// --- Scenarios ------------------------------------------------------------------------

async function screens() {
  // Webviews created before CDP attached (the first-run setup page) are not
  // visible to the driver: start from a clean editor area.
  await command("workbench.action.closeAllEditors");
  const setup = async () => {
    await command("russet.openSetup");
    const f = await webview("Set up Russet");
    await waitFor(async () => (await f.locator("#summary").textContent())?.includes("steps done"), "setup summary");
    return f;
  };
  let f = await setup();
  const steps = await f.locator("ol.steps > li").count();
  check("setup page lists the runner steps plus the account step", steps === 9, `${steps} steps`);
  check("Xcode archive detected in Downloads", (await f.locator("#step-xcode-sdk").textContent()).includes("Xcode 27.2"));
  check("virtual iPhone defaults to iPhone17,3 / 27.0.1",
    (await f.locator("#virtual-device").inputValue()) === "iPhone17,3" && (await f.locator("#virtual-ios").inputValue()) === "27.0.1");
  check("iPhone step hands off to the device picker", (await f.locator("#step-iphone").textContent()).includes("Choose iPhone"));
  await shot("01-setup-dark", "Guided setup, dark theme");

  await f.locator("#step-xcode-sdk button", { hasText: "Set up" }).click();
  await waitFor(async () => (await f.locator("#step-xcode-sdk progress").count()) > 0, "xcode-sdk progress");
  await sleep(1300);
  await f.locator("#step-xcode-sdk details summary").click().catch(() => {});
  await shot("02-setup-running", "Xcode step running: progress bar, log tail, Cancel");
  await waitFor(async () => (await f.locator("#step-xcode-sdk .badge").textContent()) === "Done", "xcode-sdk done", 30000);
  check("Xcode step ran to Done through the runner", true);

  // Native sign-in.
  await f.locator("#step-account button", { hasText: "Sign in with Apple ID" }).click();
  let s = await webview("Sign in with Apple ID");
  await waitFor(() => s.locator("#agree").count(), "consent screen");
  await shot("03-signin-consent", "Apple ID sign-in: consent");
  await s.locator("#agree").check();
  await s.locator("button[type=submit]").click();
  await waitFor(() => s.locator("#apple-id").count(), "credentials form");
  await s.locator("#apple-id").fill("synthetic.developer@example.invalid");
  await s.locator("#password").fill("synthetic-password");
  await shot("04-signin-credentials", "Apple ID and password (masked)");
  await s.locator("button[type=submit]").click();
  await waitFor(() => s.locator("#code").count(), "two-factor screen");
  check("password field cleared after sending", true);
  await shot("05-signin-code", "Two-factor code from a trusted device");
  await s.locator("button", { hasText: "Text me instead" }).click();
  await waitFor(() => s.locator("input[name=phone]").count(), "trusted numbers");
  await shot("06-signin-text-me", "Text me instead: trusted numbers");
  await s.locator("button[type=submit]").click();
  await waitFor(() => s.locator("#code").count(), "SMS code screen");
  await s.locator("#code").fill("123456");
  await waitFor(async () => (await s.locator("dl").count()) > 0, "signed-in screen");
  check("sign-in shows the account and team", (await s.locator("dl").textContent()).includes("SYNTHTEAM1"));
  await shot("07-signin-done", "Signed in: account and team");
  await s.locator("button", { hasText: "Done" }).click();

  // Virtual iPhone before it exists.
  await command("russet.openVirtualIphone");
  let v = await webview("Virtual iPhone");
  await waitFor(async () => (await v.locator(".screen-overlay").textContent())?.includes("No virtual iPhone"), "not-created state");
  await shot("08-virtual-iphone-not-created", "Virtual iPhone tab before the VM exists");

  // Create it through the setup page (fake runner): vphone, then virtual-iphone.
  f = await setup();
  await f.locator("#run-all").click();
  await waitFor(async () => (await f.locator("#step-virtual-iphone .badge").textContent()) === "Done", "virtual iPhone created", 60000);
  check("Set up remaining steps ran vphone and virtual-iphone", true);
  await shot("09-setup-complete", "All setup steps done");

  // Xcode-style destination picker with physical and virtual iPhones.
  await command("russet.selectDevice", [], { noWait: true });
  await sleep(700);
  const pickerText = await page.locator(".quick-input-widget").textContent();
  check("destination picker lists iPhones and Virtual iPhones", pickerText.includes("Virtual iPhones") && pickerText.includes("iPhone 15"), pickerText.slice(0, 160));
  await shot("10-destination-picker", "Run destination picker: USB and virtual iPhones");
  await page.keyboard.type("iPhone 16 Pro");
  await sleep(300);
  await page.keyboard.press("Enter");
  await sleep(500);
  const status = await page.locator("#workbench\\.parts\\.statusbar").textContent();
  check("status bar shows the virtual destination", status.includes("iPhone 16 Pro"), status.slice(0, 200));

  await command("russet.openVirtualIphone");
  v = await webview("Virtual iPhone");
  await waitFor(async () => (await v.locator(".screen-overlay").textContent())?.includes("not running"), "not-running state");
  const box = await v.locator(".screen").boundingBox();
  await v.locator(".screen").click({ position: { x: box.width / 2, y: box.height / 3 } });
  await v.locator(".screen").press("a");
  await waitFor(async () => (await v.locator(".note").textContent())?.includes("captured"), "input note");
  check("touch and keys are captured and the stub says why they are not delivered",
    (await v.locator(".note").textContent()).includes("Not delivered"));
  await shot("11-virtual-iphone", "Virtual iPhone tab: frame, hardware buttons, Run/Debug in the status bar");
  await command("russet.virtual.rotateRight");
  await sleep(300);
  await shot("12-virtual-iphone-landscape", "Rotated to landscape");
  await command("russet.virtual.rotateLeft");
  await command("russet.openMirror");
  const m = await webview("iPhone");
  await waitFor(async () => (await m.locator(".screen-overlay").textContent())?.includes("iPhone 16 Pro"), "sidebar mirror state");
  check("sidebar iPhone view follows the run destination", true);
  await shot("12b-sidebar-mirror", "Sidebar iPhone view with the shared device frame");

  const run = await command("russet.run").then(() => true, () => false);
  check("Run on a not-yet-runnable virtual iPhone explains instead of building", run);

  // Light and high-contrast themes.
  await command("__setting", ["workbench.colorTheme", "Default Light Modern"]);
  await sleep(800);
  await shot("13-virtual-iphone-light", "Light theme");
  f = await setup();
  await shot("14-setup-light", "Setup page, light theme");
  await command("__setting", ["workbench.colorTheme", "Default High Contrast"]);
  await sleep(800);
  await shot("15-setup-high-contrast", "Setup page, high-contrast theme");
  await command("russet.openVirtualIphone");
  await sleep(500);
  await shot("16-virtual-iphone-high-contrast", "Virtual iPhone, high-contrast theme");
}

async function liveRun() {
  const ready = JSON.parse(fs.readFileSync(path.join(bridge, "ready.json"), "utf8"));
  check("extension connected to the managed backend", ready.state === "connected", ready.state);
  await command("workbench.action.closeAllEditors");
  await command("russet.openSetup");
  const f = await webview("Set up Russet", 40000);
  await waitFor(async () => (await f.locator("ol.steps > li").count()) >= 8, "runner status in the page", 60000);
  const text = await f.locator("main").textContent();
  check("setup page shows the real runner's steps", text.includes("Windows Subsystem for Linux") && text.includes("Virtual iPhone engine"));
  check("runner status read without errors", !(await f.locator(".alert[role=alert]").count()));
  await shot("live-01-setup", "Setup page against the real runner (status only)");
  await command("russet.account.signInAppleId");
  const s = await webview("Sign in with Apple ID");
  await waitFor(async () => !(await s.locator("main").textContent()).includes("Checking"), "sign-in probe", 30000);
  const signinText = await s.locator("main").textContent();
  check("sign-in page answers from the engine", signinText.length > 0, signinText.slice(0, 160));
  await shot("live-02-signin", "Sign-in page against the current engine");
  await command("russet.openVirtualIphone");
  await webview("Virtual iPhone");
  await shot("live-03-virtual-iphone", "Virtual iPhone tab against the real runner state");
}

// --- Run ------------------------------------------------------------------------------

const driver = (async () => {
  await waitFor(() => fs.existsSync(path.join(bridge, "ready.json")), "the extension to activate", 120000);
  const ready = JSON.parse(fs.readFileSync(path.join(bridge, "ready.json"), "utf8"));
  check("development extension loaded from this worktree", path.resolve(ready.extensionPath).toLowerCase() === ext.toLowerCase(), ready.extensionPath);
  check("W19 commands contributed", ready.missing.length === 0, ready.missing.join(", "));
  await connect();
  try {
    await (live ? liveRun() : screens());
  } catch (err) {
    await shot("zz-failure", String(err)).catch(() => {});
    if (page) {
      for (const f of page.frames()) {
        console.log(`frame ${f.url().slice(0, 100)} | ${await f.title().catch(() => "?")}`);
      }
    }
    throw err;
  } finally {
    await command("__done").catch(() => {});
  }
})();

let failed = false;
try {
  await Promise.all([
    runTests({
      vscodeExecutablePath: editorExe,
      extensionDevelopmentPath: ext,
      extensionTestsPath: path.join(here, "suite.cjs"),
      extensionTestsEnv: env,
      launchArgs: [
        path.join(ext, "test", "fixtures", "HelloProject"),
        "--user-data-dir", path.join(runtime, "user"),
        "--extensions-dir", path.join(runtime, "extensions"),
        "--skip-welcome", "--skip-release-notes", "--disable-workspace-trust", "--disable-updates",
        `--remote-debugging-port=${port}`,
      ],
    }),
    driver,
  ]);
} catch (err) {
  failed = true;
  console.error(err);
} finally {
  for (const c of cleanups.reverse()) {
    try {
      await c();
    } catch {
      /* ignore */
    }
  }
}
fs.writeFileSync(path.join(runtime, "results.json"), JSON.stringify(results, null, 2));
const bad = results.checks.filter((c) => !c.ok);
console.log(`${results.checks.length - bad.length}/${results.checks.length} checks passed; ${results.screenshots.length} screenshots in ${shots}`);
process.exit(failed || bad.length ? 1 : 0);
