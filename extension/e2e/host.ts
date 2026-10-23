// Checks that `localpass.exe` works as the browser's native-messaging helper: started the way a
// browser starts it (extension origin as the first argument, frames on stdin/stdout), it relays to
// a broker. Uses the dev host's private pipe name, so a running LocalPass is not touched.
//
//   cargo build -p lp-broker --examples && cargo build -p localpass --features custom-protocol
//   bun run e2e/host.ts [path\to\localpass.exe]      (use the release exe to test the GUI build)

import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "../..");
const appExe = process.argv[2] ?? join(root, "target/debug/localpass.exe");
const devHostExe = join(root, "target/debug/examples/dev_host.exe");
const EXTENSION_ID = "khlgdnhadbjfghhonaohnhdmedgnenlc";
const profile = `hosttest${process.pid}`;
const env = { ...process.env, LOCALPASS_PROFILE: profile };

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures++;
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** A helper process with framed JSON on its stdio, like the browser sees it. */
function startHelper(origin: string) {
  const child = Bun.spawn([appExe, origin, "--parent-window=0"], { env, stdin: "pipe", stdout: "pipe", stderr: "ignore" });
  const reader = child.stdout.getReader();
  let buffer = new Uint8Array(0);
  const fill = async (n: number): Promise<boolean> => {
    while (buffer.length < n) {
      const { done, value } = await reader.read();
      if (done) return false;
      const next = new Uint8Array(buffer.length + value.length);
      next.set(buffer);
      next.set(value, buffer.length);
      buffer = next;
    }
    return true;
  };
  return {
    child,
    send(message: unknown) {
      const body = new TextEncoder().encode(JSON.stringify(message));
      const frame = new Uint8Array(4 + body.length);
      new DataView(frame.buffer).setUint32(0, body.length, true);
      frame.set(body, 4);
      child.stdin.write(frame);
      child.stdin.flush();
    },
    async recv(timeoutMs = 20000): Promise<any | null> {
      const result = await Promise.race([
        (async () => {
          if (!(await fill(4))) return null;
          const length = new DataView(buffer.buffer, buffer.byteOffset).getUint32(0, true);
          if (!(await fill(4 + length))) return null;
          const body = buffer.slice(4, 4 + length);
          buffer = buffer.slice(4 + length);
          return JSON.parse(new TextDecoder().decode(body));
        })(),
        sleep(timeoutMs).then(() => "timeout" as const),
      ]);
      if (result === "timeout") throw new Error("the helper did not answer in time");
      return result;
    },
    async finish(): Promise<number> {
      child.stdin.end();
      return child.exited;
    },
  };
}

const host = Bun.spawn([devHostExe], { env, stdin: "pipe", stdout: "pipe", stderr: "inherit" });
try {
  // Wait for the dev host to listen. Keep reading its output afterwards: a closed pipe would
  // make its next print fail.
  const decoder = new TextDecoder();
  let seen = "";
  void (async () => {
    for await (const chunk of host.stdout) seen += decoder.decode(chunk);
  })();
  for (let i = 0; i < 150 && !seen.includes("READY"); i++) await sleep(100);
  check("the dev host is listening", seen.includes("READY"));

  // The browser's helper path: relays frames to the broker.
  const helper = startHelper(`chrome-extension://${EXTENSION_ID}/`);
  helper.send({ type: "hello", v: 1, name: "Chrome extension", kind: "extension" });
  check("the helper relays the handshake", (await helper.recv())?.type === "unpaired");
  helper.send({ type: "pair" });
  check("pairing code comes back", (await helper.recv())?.type === "pairing");
  check("the browser is paired", (await helper.recv())?.type === "paired");
  helper.send({ id: 1, op: "status" });
  const status = await helper.recv();
  check("requests work through the helper", status?.id === 1 && status?.result?.status === "unlocked");
  check("closing the browser's side ends the helper", (await helper.finish()) === 0);

  // Any other extension is refused.
  const stranger = startHelper("chrome-extension://abcdefghijklmnopabcdefghijklmnop/");
  const refusal = await stranger.recv();
  check("other extensions are refused", refusal?.type === "hostError" && refusal?.code === "forbidden", JSON.stringify(refusal));
  check("the refused helper exits with an error", (await stranger.finish()) === 1);
} catch (e) {
  console.error(e);
  failures++;
} finally {
  host.stdin.write("quit\n");
  host.stdin.flush();
  host.kill();
}

if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("the helper works as the browser's native-messaging host");
