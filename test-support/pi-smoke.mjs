import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, lstatSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const directory = mkdtempSync(join(tmpdir(), "mcpd-smoke-"));
const previousConfig = process.env.XDG_CONFIG_HOME;
const previousPi = process.env.PI_CODING_AGENT_DIR;
process.env.XDG_CONFIG_HOME = join(directory, "config");
process.env.PI_CODING_AGENT_DIR = join(directory, "pi");
const registered = new Map();
const events = new Map();
const run = (...args) => execFileSync("mcpd", args, { encoding: "utf8" });
const decode = (result) => JSON.parse(result.content[0].text);

try {
  mkdirSync(join(process.env.PI_CODING_AGENT_DIR, "extensions"), { recursive: true });
  const extension = join(process.env.PI_CODING_AGENT_DIR, "extensions/mcpd.ts");
  const source = resolve("integrations/pi.ts");
  const original = readFileSync(source, "utf8");
  symlinkSync(source, extension);
  run("setup", "pi");
  assert(!lstatSync(extension).isSymbolicLink());
  assert.equal(readFileSync(source, "utf8"), original);
  run("setup", "pi");
  const { default: load } = await import(pathToFileURL(extension).href);
  load({
    registerTool(tool) { registered.set(tool.name, tool); },
    on(name, handler) { events.set(name, handler); },
  });
  assert.deepEqual([...registered.keys()].sort(), ["mcpd_find_tools", "mcpd_list_tools", "mcpd_use_tool"]);
  const call = (name, args) => registered.get(name).execute("smoke", args);
  const empty = decode(await call("mcpd_find_tools", {}));
  assert.deepEqual(empty.servers, []);
  assert.deepEqual(empty.tools, []);
  run("register", "mock", resolve("target/debug/mock-mcp-server"));
  const found = decode(await call("mcpd_find_tools", { query: "echo" }));
  assert.equal(found.tools[0].name, "mock__echo");
  assert.equal(found.total_matches, 1);
  assert.deepEqual(found.errors, []);
  assert.deepEqual(decode(await call("mcpd_use_tool", {
    tool_name: found.tools[0].name,
    arguments: { message: "installed bridge works" },
  })), { message: "installed bridge works" });
  await assert.rejects(call("mcpd_use_tool", { tool_name: "mock__fail" }), /intentional failure/);
  run("unregister", "mock");
  assert.deepEqual(decode(await call("mcpd_find_tools", {})).servers, []);
  console.log("Installed Pi bridge: setup, empty registry, hot registration, discovery, invocation, errors, and removal passed.");
} finally {
  await events.get("session_shutdown")?.();
  if (previousConfig === undefined) delete process.env.XDG_CONFIG_HOME;
  else process.env.XDG_CONFIG_HOME = previousConfig;
  if (previousPi === undefined) delete process.env.PI_CODING_AGENT_DIR;
  else process.env.PI_CODING_AGENT_DIR = previousPi;
  rmSync(directory, { recursive: true, force: true });
}
