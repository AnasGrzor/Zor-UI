// Installed by zorui --install-agents. Uses OpenCode's native question reply API.
import { spawn } from "node:child_process";

const executable = __ZORUI_EXECUTABLE__;

export default async function ZorIsland(ctx) {
  let client = ctx.client;
  if (!client.question) {
    const { createOpencodeClient } = await import("@opencode-ai/sdk/v2");
    client = createOpencodeClient({
      ...ctx.client._client?.getConfig?.(),
      baseUrl: ctx.serverUrl.toString(),
      directory: ctx.directory,
    });
  }
  const pending = new Map();
  const report = (payload) => {
    const child = spawn(executable, ["--agent", "opencode"], {
      windowsHide: true,
      stdio: ["pipe", "pipe", "ignore"],
    });
    child.on("error", () => {}); // Keep OpenCode's own prompt available if Zor is unavailable.
    child.stdin.on("error", () => {});
    child.stdin.end(JSON.stringify({ cwd: ctx.directory, ...payload }));
    return child;
  };
  const finish = (requestID, state = "done") => {
    const request = pending.get(requestID);
    if (!request) return;
    pending.delete(requestID);
    request.child.kill();
    report({ session_id: request.session, state });
  };
  return {
    event: async ({ event }) => {
      const p = event.properties || {};
      if (event.type === "question.replied" || event.type === "question.rejected") {
        finish(p.requestID);
      } else if (event.type === "question.asked" && p.id && p.sessionID && Array.isArray(p.questions)) {
        if (pending.has(p.id)) return;
        const session = `${p.sessionID}:${p.id}`;
        const child = report({
          hook_event_name: "IslandQuestion",
          session_id: session,
          tool_name: "question",
          tool_input: { questions: p.questions },
        });
        pending.set(p.id, { child, session, sessionID: p.sessionID });
        let output = "";
        child.stdout.on("data", (data) => { output += data; });
        child.on("close", () => {
          // The user may already have answered in OpenCode while the island was open.
          if (pending.get(p.id)?.child !== child) return;
          pending.delete(p.id);
          void (async () => {
            try {
              const result = JSON.parse(output);
              if (!Array.isArray(result.answers) || result.answers.length !== p.questions.length) return;
              await client.question.reply({ requestID: p.id, directory: ctx.directory, answers: result.answers }, { throwOnError: true });
            } catch {
              // A timeout, unavailable shell, or an answer in OpenCode leaves its native flow intact.
            } finally {
              report({ session_id: session, state: "done" });
            }
          })();
        });
      } else if (event.type === "session.deleted" || event.type === "session.error") {
        const sessionID = p.sessionID || p.info?.id;
        for (const [id, request] of pending) {
          if (request.sessionID === sessionID) finish(id, "ended");
        }
      } else if (event.type === "server.instance.disposed") {
        for (const id of pending.keys()) finish(id, "ended");
      }
    },
  };
}
