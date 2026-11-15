import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { PassThrough, Writable } from "node:stream";
import { readFile } from "node:fs/promises";

const children = [];
mock.module("node:child_process", {
  namedExports: {
    spawn(exe, args, options) {
      const child = new EventEmitter();
      child.exe = exe;
      child.args = args;
      child.options = options;
      child.input = "";
      child.stdout = new PassThrough();
      child.stdin = new Writable({ write(data, encoding, next) { child.input += data; next(); } });
      child.kill = () => { child.killed = true; child.emit("close"); };
      children.push(child);
      return child;
    },
  },
});
const template = await readFile(new URL("../native/zor-ui/src/bridge/opencode_island.js", import.meta.url), "utf8");
const source = template.replace("__ZORUI_EXECUTABLE__", JSON.stringify("C:/Program Files/Zor/zorui.exe"));
const { default: plugin } = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
const tick = () => new Promise((resolve) => setImmediate(resolve));

async function setup() {
  const replies = [];
  const hooks = await plugin({
    directory: "D:/project with spaces",
    client: { question: { reply: async (...args) => { replies.push(args); } } },
  });
  const request = {
    type: "question.asked",
    properties: {
      id: `request-${children.length}`,
      sessionID: "session-1",
      questions: [{ question: "Which targets?", multiple: true, options: [{ label: "Windows, desktop" }, { label: "Linux" }] }],
    },
  };
  await hooks.event({ event: request });
  return { hooks, replies, request, child: children.at(-1) };
}

test("OpenCode forwards questions and replies using its native API", async () => {
  const { replies, request, child } = await setup();
  assert.equal(child.options.windowsHide, true);
  assert.equal(child.options.shell, undefined);
  assert.deepEqual(child.args, ["--agent", "opencode"]);
  const payload = JSON.parse(child.input);
  assert.equal(payload.hook_event_name, "IslandQuestion");
  assert.equal(payload.session_id, `session-1:${request.properties.id}`);
  assert.deepEqual(payload.tool_input.questions, request.properties.questions);
  child.stdout.write(JSON.stringify({ answers: [["Windows, desktop", "Linux"]] }));
  child.emit("close");
  await tick();
  assert.deepEqual(replies[0][0], {
    requestID: request.properties.id,
    directory: "D:/project with spaces",
    answers: [["Windows, desktop", "Linux"]],
  });
  assert.equal(JSON.parse(children.at(-1).input).state, "done");
});

test("an answer in OpenCode cancels the waiting island and cannot send a second reply", async () => {
  const { hooks, replies, request, child } = await setup();
  await hooks.event({ event: { type: "question.replied", properties: { requestID: request.properties.id } } });
  assert.equal(child.killed, true);
  child.stdout.write(JSON.stringify({ answers: [["Linux"]] }));
  child.emit("close");
  await tick();
  assert.equal(replies.length, 0);
});

test("a timeout keeps OpenCode's original prompt available", async () => {
  const { replies, child } = await setup();
  child.emit("close");
  await tick();
  assert.equal(replies.length, 0);
  assert.equal(JSON.parse(children.at(-1).input).state, "done");
});

test("disposing a server clears all its pending island questions", async () => {
  const { hooks, child } = await setup();
  await hooks.event({ event: { type: "server.instance.disposed", properties: {} } });
  assert.equal(child.killed, true);
  assert.equal(JSON.parse(children.at(-1).input).state, "ended");
});
