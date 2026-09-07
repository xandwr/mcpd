import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface } from "node:readline";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  let child: ChildProcessWithoutNullStreams | undefined;
  let ready: Promise<void> | undefined;
  let sequence = 0;
  let stderr = "";
  const pending = new Map<number, { resolve: (value: any) => void; reject: (error: Error) => void }>();

  function stop(error = new Error("mcpd connection closed")) {
    const process = child;
    child = undefined;
    ready = undefined;
    for (const request of pending.values()) request.reject(error);
    pending.clear();
    if (process) {
      process.stdin.end();
      const timer = setTimeout(() => process.kill("SIGKILL"), 2000);
      timer.unref();
      process.once("close", () => clearTimeout(timer));
    }
  }

  function send(message: object) {
    if (!child) throw new Error("mcpd is disconnected");
    child.stdin.write(JSON.stringify(message) + "\n");
  }

  function request(method: string, params: object, signal?: AbortSignal): Promise<any> {
    signal?.throwIfAborted();
    const id = ++sequence;
    return new Promise((resolve, reject) => {
      const finish = (error?: Error, result?: any) => {
        clearTimeout(timer);
        signal?.removeEventListener("abort", abort);
        pending.delete(id);
        if (error) reject(error);
        else resolve(result);
      };
      const abort = () => stop(new Error("mcpd request aborted"));
      const timer = setTimeout(() => stop(new Error(`mcpd ${method} timed out`)), 120000);
      pending.set(id, { resolve: (value) => finish(undefined, value), reject: (error) => finish(error) });
      signal?.addEventListener("abort", abort, { once: true });
      try {
        send({ jsonrpc: "2.0", id, method, params });
      } catch (error) {
        finish(error as Error);
      }
    });
  }

  async function connect() {
    if (ready) return ready;
    stderr = "";
    const process = spawn("mcpd", ["serve"], { stdio: "pipe" });
    child = process;
    process.stderr.on("data", (data) => { stderr = (stderr + data).slice(-8192); });
    process.stdin.on("error", (error) => { if (child === process) stop(error); });
    process.on("error", (error) => { if (child === process) stop(error); });
    process.on("close", (code) => {
      if (child === process) stop(new Error(`mcpd exited (${code}): ${stderr}`));
    });
    const lines = createInterface({ input: process.stdout });
    lines.on("line", (line) => {
      if (child !== process) return;
      try {
        const message = JSON.parse(line);
        if (message.method) {
          if (message.id !== undefined) {
            send(message.method === "ping"
              ? { jsonrpc: "2.0", id: message.id, result: {} }
              : { jsonrpc: "2.0", id: message.id, error: { code: -32601, message: "Unsupported method" } });
          }
          return;
        }
        const waiting = pending.get(message.id);
        if (message.error) waiting?.reject(new Error(message.error.message));
        else waiting?.resolve(message.result);
      } catch (error) {
        stop(error as Error);
      }
    });
    ready = request("initialize", {
      protocolVersion: "2025-11-25",
      capabilities: {},
      clientInfo: { name: "pi-mcpd", version: "1.0.0" },
    }).then(() => { send({ jsonrpc: "2.0", method: "notifications/initialized" }); }).catch((error) => {
      if (child === process) stop(error);
      throw error;
    });
    return ready;
  }

  for (const name of ["find_tools", "list_tools", "use_tool"] as const) {
    pi.registerTool({
      name: `mcpd_${name}`,
      label: `mcpd ${name}`,
      description: name === "find_tools"
        ? "Search locally registered MCP capabilities before deciding you lack a tool for a task. Matches keywords in server names, tool names, and descriptions. Returns bounded results with input schemas, server names, and backend errors. Omit query to browse; use server to scope discovery."
        : name === "list_tools"
        ? "List the complete tool catalog from all registered mcpd servers with input schemas. Use mcpd_find_tools for a focused search."
        : "Invoke a backend tool using its server__tool name and arguments from mcpd_find_tools or mcpd_list_tools.",
      parameters: (name === "find_tools"
        ? {
            type: "object",
            properties: {
              query: { type: "string", description: "Space-separated keywords. Matches any keyword; names rank above descriptions." },
              server: { type: "string", description: "Optional exact registered server name." },
              limit: { type: "integer", minimum: 1, maximum: 100, default: 10 },
            },
            additionalProperties: false,
          }
        : name === "list_tools"
        ? { type: "object", properties: {}, additionalProperties: false }
        : {
            type: "object",
            properties: { tool_name: { type: "string" }, arguments: { type: "object", additionalProperties: true } },
            required: ["tool_name"],
            additionalProperties: false,
          }) as any,
      async execute(_id, params, signal) {
        signal?.throwIfAborted();
        await connect();
        const result = await request("tools/call", { name, arguments: params }, signal);
        const content = (result.content ?? []).map((item: any) =>
          item.type === "text" || item.type === "image" ? item : { type: "text", text: JSON.stringify(item) });
        if (result.isError) throw new Error(content.map((item: any) => item.text ?? JSON.stringify(item)).join("\n"));
        return { content, details: result };
      },
    });
  }

  pi.on("session_shutdown", async () => { stop(); });
}
