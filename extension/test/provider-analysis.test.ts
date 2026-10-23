// The pure half of the assisted Windows provider test (e2e/provider-analysis.ts): log parsing, the
// PASS/FAIL judgement of one ceremony and the result table. The sample lines have the real formats
// (crates/lp-winauth/src/log.rs, app/src-tauri/src/hello.rs).

import { describe, expect, test } from "bun:test";
import { analysePrompts, analyseProvider, failureCount, formatTable, helloCount, judgePhase, parseProviderStatus, providerLines, type JudgeInput, type PhaseSpec, type Row } from "../e2e/provider-analysis";
import type { RpEvent } from "../e2e/rp";

const T = "pid=4304 tid=20688 pfn=LocalPass.Passkeys_rwhsg7s1qxd2y";
const line = (time: string, text: string) => `2026-10-21 ${time} ${T} ${text}`;
const TX = "8D3860D3-0253-4FDD-9B81-C386AC7B2FFC";

const goodRegistration = [
  line("12:40:49.100", `MakeCredential tx=${TX} hWnd=0x508ca cbReq=179 cbSig=96`),
  line("12:40:49.101", "request signature ok (Sha256)"),
  line("12:40:51.900", "the app created credential 1a2b3c4d"),
  line("12:40:51.950", "credential cache: added 1a2b3c4d"),
  line("12:40:51.980", `tx=${TX} finished hr=0x00000000 responseBytes=312`),
].join("\n");

const oneHello = [
  "2026-10-21T05:40:49.500000Z  INFO passkey: Windows Hello starting; foreground: hwnd=0x1 pid=2 exe=msedge.exe; Windows Security windows already open: 0",
  "2026-10-21T05:40:49.760000Z  INFO passkey: Hello dialog appeared after 260 ms (class=Credential Dialog Xaml Host)",
  "2026-10-21T05:40:49.780000Z  INFO passkey: Hello dialog in front: true",
].join("\n");

describe("provider.log", () => {
  test("lines carry their local time and message, other lines are ignored", () => {
    const lines = providerLines(`${line("12:40:49.100", "hello")}\r\nnot a log line\r\n${line("12:40:50.100", "again")}\r\n`);
    expect(lines.map((l) => l.text)).toEqual(["hello", "again"]);
    expect(lines[1]!.time - lines[0]!.time).toBe(1000);
  });

  test("a clean create: one transaction, hr 0, no non-zero hr anywhere", () => {
    const a = analyseProvider(goodRegistration);
    expect(a.txs).toHaveLength(1);
    expect(a.txs[0]).toMatchObject({ id: TX, op: "MakeCredential", hr: "0x00000000", repeats: 0 });
    expect(a.txs[0]!.finished! - a.txs[0]!.first).toBe(2880);
    expect(a.nonZero).toEqual([]);
  });

  test("a failed call shows up as non-zero hr on the operation and the transaction", () => {
    const a = analyseProvider(
      [line("12:41:10.480", `GetAssertion tx=${TX} hWnd=0x1 cbReq=10 cbSig=96`), line("12:41:10.485", "operation failed: cancelled or not approved -> hr=0x80090036"), line("12:41:10.486", `tx=${TX} finished hr=0x80090036 responseBytes=0`)].join("\n"),
    );
    expect(a.txs[0]).toMatchObject({ op: "GetAssertion", hr: "0x80090036" });
    expect(a.nonZero).toHaveLength(2);
  });

  test("Windows repeating a call with the same transaction id is counted, not doubled", () => {
    const text = [line("12:40:49.100", `MakeCredential tx=${TX} hWnd=0x1 cbReq=1 cbSig=1`), line("12:40:49.900", `MakeCredential tx=${TX} hWnd=0x1 cbReq=1 cbSig=1`), line("12:40:49.901", `tx=${TX} was seen before: returning the first call's result`), line("12:40:50.000", `tx=${TX} finished hr=0x00000000 responseBytes=5`)].join("\n");
    const a = analyseProvider(text);
    expect(a.txs).toHaveLength(1);
    expect(a.txs[0]!.repeats).toBe(1);
  });

  test("the runner's own `status` calls and a good COM start are noise, a failing one is not", () => {
    const noise = [line("12:00:00.000", "start mode=status exe=C:\\x\\lp-winauth-provider.exe"), line("12:00:00.001", "CoInitializeEx(MTA) hr=0x00000000")].join("\n");
    expect(analyseProvider(noise).lines).toHaveLength(0);
    const bad = analyseProvider(line("12:00:00.001", "CoInitializeEx(STA) hr=0x80010106"));
    expect(bad.nonZero).toHaveLength(1);
  });

  test("hr values compare numerically, not as text (lower case, short)", () => {
    expect(analyseProvider(line("12:00:00.001", "something hr=0x0")).nonZero).toEqual([]);
    expect(analyseProvider(line("12:00:00.001", "something hr=0x8009000f")).nonZero).toHaveLength(1);
  });

  test("CancelOperation lines and the app start are picked out", () => {
    const a = analyseProvider(
      [
        line("12:00:00.001", "LocalPass is not running: starting C:\\app\\localpass.exe"),
        line("12:00:00.900", "started C:\\app\\localpass.exe with createprocess (pid 1234)"),
        line("12:00:03.000", `CancelOperation tx=${TX}: cancelling; signature covers request`),
      ].join("\n"),
    );
    expect(a.starting).toBe(true);
    expect(a.started).toContain("(pid 1234)");
    expect(a.cancelOps).toHaveLength(1);
  });
});

describe("prompts.log", () => {
  test("one Hello check: one start and one dialog", () => {
    const p = analysePrompts(oneHello);
    expect(p).toMatchObject({ starts: 1, dialogs: 1, dialogMs: [260], withdrawn: 0, left: 0 });
    expect(helloCount(p)).toBe(1);
  });

  test("a cancel is counted, a dialog left open after it is noticed, a second check is a second start", () => {
    const p = analysePrompts(
      [
        oneHello,
        "2026-10-21T05:40:55.000000Z  INFO Windows Hello check cancelled by the caller; Cancel() -> ok",
        "2026-10-21T05:40:55.400000Z  INFO Hello dialog still open 400 ms after Cancel(): true (closed it)",
        "2026-10-21T05:40:56.000000Z  INFO passkey: Windows Hello starting; foreground: x; Windows Security windows already open: 0",
      ].join("\n"),
    );
    expect(p).toMatchObject({ starts: 2, withdrawn: 1, left: 1 });
    expect(helloCount(p)).toBe(2);
  });

  test("an empty log means no Hello", () => {
    expect(helloCount(analysePrompts(""))).toBe(0);
  });
});

describe("status output", () => {
  const enabled = `LocalPass passkey provider, class {91E9966B-58AE-455A-AB60-4B2FDB66A730}
package identity: LocalPass.Passkeys_rwhsg7s1qxd2y
registered: yes, enabled
operation-signing key: ECDSA P-384 (104 bytes)
pipe profile: none (the real app)
paired: yes
app reachable: yes
vault: Unlocked
credential cache: 2 row(s)`;

  test("enabled, paired, reachable", () => {
    expect(parseProviderStatus(enabled)).toMatchObject({ registered: true, enabled: true, paired: true, appReachable: true, vault: "Unlocked", cacheRows: 2, profile: "none (the real app)" });
  });

  test("disabled, not paired, app not running", () => {
    const s = parseProviderStatus("registered: yes, disabled\npaired: no\napp reachable: no (LocalPass is not running)\ncredential cache: 0 row(s)");
    expect(s).toMatchObject({ registered: true, enabled: false, paired: false, appReachable: false, cacheRows: 0 });
    expect(s.vault).toBeUndefined();
  });

  test("not registered", () => {
    expect(parseProviderStatus("registered: no")).toMatchObject({ registered: false, enabled: false });
  });
});

// ---------------------------------------------------------------------------------------------

const okChecks = [
  { name: "origin, RP id hash, challenge and type verified", ok: true },
  { name: "user verification flag (UV)", ok: true },
];
const registerEvent: RpEvent = { seq: 1, time: "t", kind: "register", verified: true, verifyMs: 12, aaguid: "00000000-0000-0000-0000-000000000000", fmt: "none", flags: { up: true, uv: true, be: true, bs: true }, counter: 0, checks: okChecks };
const signinEvent: RpEvent = { seq: 2, time: "t", kind: "signin", verified: true, verifyMs: 3, flags: { up: true, uv: true, be: true, bs: true }, counter: 0, userHandleMatches: true, checks: okChecks };
const clientError = (errorName: string): RpEvent => ({ seq: 3, time: "t", kind: "client", action: "get", outcome: "error", errorName, errorMessage: "The operation either timed out or was not allowed.", pageMs: 4000 });

const register: PhaseSpec = { id: "register", action: "register", outcome: "success" };
const signin: PhaseSpec = { id: "signin", action: "signin", outcome: "success" };
const cancel: PhaseSpec = { id: "cancel", action: "signin", outcome: "cancelled" };

const finished = (hr: string, op = "MakeCredential") => analyseProvider([line("12:00:00.000", `${op} tx=${TX} hWnd=0x1 cbReq=1 cbSig=1`), line("12:00:02.000", `tx=${TX} finished hr=${hr} responseBytes=1`)].join("\n"));

function judge(over: Partial<JudgeInput>) {
  return judgePhase({ phase: register, result: { kind: "event", event: registerEvent }, provider: finished("0x00000000"), hello: analysePrompts(oneHello), virtual: false, pageMs: 5000, phaseMs: 9000, ...over });
}
const byName = (rows: Row[], part: string) => rows.find((r) => r.name.includes(part));

describe("judging a ceremony", () => {
  test("everything right: registration verified, hr 0, one Hello", () => {
    const j = judge({});
    expect(j.registered).toBe(true);
    expect(j.rows.filter((r) => r.status === "FAIL")).toEqual([]);
    expect(byName(j.rows, "registration verified by the RP")?.status).toBe("PASS");
    expect(byName(j.rows, "every hr= is 0")?.status).toBe("PASS");
    expect(byName(j.rows, "exactly 1 Hello")?.status).toBe("PASS");
    expect(byName(j.rows, "timing")?.detail).toContain("provider 2.0 s");
    expect(byName(j.rows, "registration verified by the RP")?.detail).toContain("flags UP=1 UV=1 BE=1 BS=1");
  });

  test("the relying party's own failed check fails the row and names the check", () => {
    const bad: RpEvent = { ...registerEvent, verified: true, flags: { up: true, uv: true, be: false, bs: false }, checks: [...okChecks, { name: "backup flags BE=1 BS=1 (LocalPass design)", ok: false, detail: "BE=0 BS=0" }] };
    const j = judge({ result: { kind: "event", event: bad } });
    expect(j.registered).toBe(false);
    expect(byName(j.rows, "registration verified by the RP")).toMatchObject({ status: "FAIL" });
    expect(byName(j.rows, "registration verified by the RP")!.detail).toContain("backup flags");
  });

  test("a registration SimpleWebAuthn rejects fails with its reason", () => {
    const j = judge({ result: { kind: "event", event: { ...registerEvent, verified: false, error: "User verification required", checks: [] } } });
    expect(byName(j.rows, "registration verified")).toMatchObject({ status: "FAIL" });
    expect(byName(j.rows, "registration verified")!.detail).toContain("User verification required");
  });

  test("an error in the page fails a sign-in that should work", () => {
    const j = judge({ phase: signin, result: { kind: "page-error", pageError: clientError("NotAllowedError") } });
    expect(byName(j.rows, "sign-in verified by the RP")).toMatchObject({ status: "FAIL" });
    expect(byName(j.rows, "sign-in verified by the RP")!.detail).toContain("NotAllowedError");
  });

  test("a non-zero hr in the provider log fails even though the site verified", () => {
    const provider = analyseProvider([line("12:00:00.000", `MakeCredential tx=${TX} hWnd=0x1 cbReq=1 cbSig=1`), line("12:00:01.000", "credential cache: add failed hr=0x8009000F"), line("12:00:02.000", `tx=${TX} finished hr=0x00000000 responseBytes=1`)].join("\n"));
    const j = judge({ provider });
    expect(byName(j.rows, "every hr= is 0")).toMatchObject({ status: "FAIL" });
  });

  test("no provider request at all is a failure with a hint in the detail", () => {
    const j = judge({ provider: analyseProvider("") });
    expect(byName(j.rows, "every hr= is 0")).toMatchObject({ status: "FAIL" });
    expect(byName(j.rows, "every hr= is 0")!.detail).toContain("saw no request");
  });

  test("two Hello prompts for one ceremony fail, none fails, in a normal phase", () => {
    const two = analysePrompts(`${oneHello}\n2026-10-21T05:40:50.000000Z  INFO again: Windows Hello starting; foreground: x; Windows Security windows already open: 0`);
    expect(byName(judge({ hello: two }).rows, "exactly 1 Hello")).toMatchObject({ status: "FAIL" });
    expect(byName(judge({ hello: analysePrompts("") }).rows, "exactly 1 Hello")).toMatchObject({ status: "FAIL" });
  });

  test("no Hello line with a locked vault or a closed app is only a warning (master password), two are still a failure", () => {
    const locked = { ...signin, id: "locked" };
    const j = judge({ phase: locked, result: { kind: "event", event: signinEvent }, hello: analysePrompts("") });
    expect(byName(j.rows, "exactly 1 Hello")).toMatchObject({ status: "WARN" });
    const two = analysePrompts(`${oneHello}\n2026-10-21T05:40:50.000000Z  INFO unlock: Windows Hello starting; foreground: x; Windows Security windows already open: 0`);
    expect(byName(judge({ phase: locked, result: { kind: "event", event: signinEvent }, hello: two }).rows, "exactly 1 Hello")).toMatchObject({ status: "FAIL" });
  });

  test("sign-in answered NTE_NOT_FOUND gets the hint about older test passkeys", () => {
    const j = judge({ phase: signin, result: { kind: "page-error", pageError: clientError("NotAllowedError") }, provider: finished("0x80090011", "GetAssertion") });
    expect(j.hints.join(" ")).toContain("Delete older test passkeys");
  });

  test("cancel: NotAllowedError, provider answered NTE_USER_CANCELLED and one Hello is a pass", () => {
    const j = judge({ phase: cancel, result: { kind: "page-error", pageError: clientError("NotAllowedError") }, provider: finished("0x80090036", "GetAssertion") });
    expect(j.rows.filter((r) => r.status === "FAIL")).toEqual([]);
    expect(byName(j.rows, "NotAllowedError")?.status).toBe("PASS");
    expect(byName(j.rows, "NTE_USER_CANCELLED")?.status).toBe("PASS");
    expect(byName(j.rows, "none after the cancel")?.status).toBe("PASS");
  });

  test("cancel: a ceremony that went through, another error name or a second prompt fail", () => {
    expect(byName(judge({ phase: cancel, result: { kind: "event", event: signinEvent } }).rows, "NotAllowedError")).toMatchObject({ status: "FAIL" });
    expect(byName(judge({ phase: cancel, result: { kind: "page-error", pageError: clientError("UnknownError") } }).rows, "NotAllowedError")).toMatchObject({ status: "FAIL" });
    const second = analysePrompts(`${oneHello}\n2026-10-21T05:40:52.000000Z  INFO again: Windows Hello starting; foreground: x; Windows Security windows already open: 0`);
    const j = judge({ phase: cancel, result: { kind: "page-error", pageError: clientError("NotAllowedError") }, provider: finished("0x80090036", "GetAssertion"), hello: second });
    expect(byName(j.rows, "none after the cancel")).toMatchObject({ status: "FAIL" });
  });

  test("closed app: the provider must have started LocalPass", () => {
    const closed = { ...signin, id: "closed" };
    const started = analyseProvider([line("12:00:00.000", `GetAssertion tx=${TX} hWnd=0x1 cbReq=1 cbSig=1`), line("12:00:00.100", "LocalPass is not running: starting C:\\a\\localpass.exe"), line("12:00:00.700", "started C:\\a\\localpass.exe with createprocess (pid 99)"), line("12:00:05.000", `tx=${TX} finished hr=0x00000000 responseBytes=1`)].join("\n"));
    const ok = judge({ phase: closed, result: { kind: "event", event: signinEvent }, provider: started, appRunningAfter: true });
    expect(byName(ok.rows, "the provider started LocalPass")?.status).toBe("PASS");
    expect(byName(ok.rows, "running afterwards")?.status).toBe("INFO");
    const notStarted = judge({ phase: closed, result: { kind: "event", event: signinEvent }, provider: finished("0x00000000", "GetAssertion"), appRunningAfter: false });
    expect(byName(notStarted.rows, "the provider started LocalPass")?.status).toBe("FAIL");
  });

  test("timeout: one failing row, and what to look at", () => {
    const j = judge({ result: { kind: "timeout", timeoutS: 240, pageEvents: [] }, provider: analyseProvider(goodRegistration) });
    expect(j.rows).toHaveLength(1);
    expect(j.rows[0]).toMatchObject({ status: "FAIL", name: "register: result" });
    expect(j.rows[0]!.detail).toContain("240 s");
    expect(j.hints.length).toBeGreaterThan(1);
  });

  test("--virtual: only the relying party is judged, the rest is skipped", () => {
    const j = judge({ virtual: true, provider: analyseProvider(""), hello: analysePrompts("") });
    expect(j.rows.map((r) => r.status)).toEqual(["PASS", "SKIP"]);
  });
});

describe("result table", () => {
  test("aligned rows, a failure count and a verdict line", () => {
    const rows = [
      { status: "PASS" as const, name: "a", detail: "fine" },
      { status: "FAIL" as const, name: "longer name", detail: "broken" },
      { status: "WARN" as const, name: "w", detail: "" },
    ];
    const text = formatTable(rows);
    expect(text[0]).toBe("PASS  a            fine");
    expect(text[1]).toBe("FAIL  longer name  broken");
    expect(text[2]).toBe("WARN  w");
    expect(text.at(-1)).toBe("1 check(s) FAILED, 1 warning(s)");
    expect(failureCount(rows)).toBe(1);
    expect(formatTable([{ status: "PASS", name: "ok", detail: "" }]).at(-1)).toBe("all checks passed");
  });
});
