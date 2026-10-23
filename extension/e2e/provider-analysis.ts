// The pure half of e2e/windows-provider.ts: reading the provider log, the app's prompts.log and the
// provider's `status` output, judging one ceremony (PASS / FAIL rows) and printing the table.
// No I/O and no side effects, so `bun test` (test/provider-analysis.test.ts) covers it.

import type { RpCheck, RpEvent } from "./rp";

export type Status = "PASS" | "FAIL" | "WARN" | "INFO" | "SKIP";
export interface Row {
  status: Status;
  name: string;
  detail: string;
}

/** NTE_USER_CANCELLED: what the provider answers when the user says no (the page sees NotAllowedError). */
export const HR_CANCELLED = "0x80090036";
/** NTE_NOT_FOUND: no passkey, or several and no way to choose. */
export const HR_NOT_FOUND = "0x80090011";

export const seconds = (ms: number | undefined) => (ms === undefined || Number.isNaN(ms) ? "?" : `${(ms / 1000).toFixed(1)} s`);

// ---------------------------------------------------------------------------------------------
// provider.log (`%LOCALAPPDATA%\LocalPass\provider\provider.log`, written by crates/lp-winauth/src/log.rs)
//   2026-10-21 21:36:07.131 pid=10628 tid=3960 pfn=LocalPass.Passkeys_rwhsg7s1qxd2y <message>

export interface LogLine {
  /** Local time in ms (the log has no zone). */
  time: number;
  text: string;
}

const LINE = /^(\d{4}-\d\d-\d\d) (\d\d:\d\d:\d\d\.\d{3}) pid=\d+ tid=\d+ pfn=\S+ (.*)$/;

export function providerLines(text: string): LogLine[] {
  const out: LogLine[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const m = LINE.exec(raw);
    if (m) out.push({ time: new Date(`${m[1]}T${m[2]}`).getTime(), text: m[3]! });
  }
  return out;
}

export interface ProviderTx {
  id: string;
  op: string;
  first: number;
  finished?: number;
  hr?: string;
  /** Windows called again with the same transaction id (the provider answers with the first result). */
  repeats: number;
}

export interface ProviderAnalysis {
  lines: LogLine[];
  txs: ProviderTx[];
  /** Every line with an `hr=` other than 0 (shortened). */
  nonZero: string[];
  cancelOps: string[];
  /** "started <exe> with <method> (pid N)": the provider launched LocalPass. */
  started?: string;
  starting: boolean;
}

/** `hr=0x...` in a log line, upper-cased: "0x80090036". */
const normaliseHr = (s: string) => `0x${s.replace(/^0[xX]/, "").toUpperCase().padStart(8, "0")}`;

export function analyseProvider(text: string): ProviderAnalysis {
  // `status` runs of the runner itself and the COM start-up line are noise unless they carry a failure.
  const lines = providerLines(text).filter((l) => !/^start mode=status|^CoInitializeEx/.test(l.text) || /hr=0x[0-9a-f]*[1-9a-f]/i.test(l.text));
  const txs = new Map<string, ProviderTx>();
  const nonZero: string[] = [];
  for (const l of lines) {
    const call = /^(MakeCredential|GetAssertion) tx=(\S+)/.exec(l.text);
    if (call) {
      const existing = txs.get(call[2]!);
      if (existing) existing.repeats++;
      else txs.set(call[2]!, { id: call[2]!, op: call[1]!, first: l.time, repeats: 0 });
    }
    const done = /^tx=(\S+) finished hr=(0x[0-9A-Fa-f]+)/.exec(l.text);
    if (done) {
      const tx = txs.get(done[1]!);
      if (tx) {
        tx.finished = l.time;
        tx.hr = normaliseHr(done[2]!);
      }
    }
    for (const h of l.text.matchAll(/hr=(0x[0-9A-Fa-f]+)/g)) {
      if (normaliseHr(h[1]!) !== "0x00000000") nonZero.push(l.text.slice(0, 110));
    }
  }
  return {
    lines,
    txs: [...txs.values()],
    nonZero,
    cancelOps: lines.filter((l) => /CancelOperation/.test(l.text)).map((l) => l.text.slice(0, 110)),
    started: lines.find((l) => /^started .* with .* \(pid \d+\)/.test(l.text))?.text,
    starting: lines.some((l) => /LocalPass is not running: starting/.test(l.text)),
  };
}

// ---------------------------------------------------------------------------------------------
// prompts.log (`%APPDATA%\app.localpass\prompts.log`, app/src-tauri/src/{hello,focus,applog}.rs)
//   2026-10-21T11:55:05.988745Z  INFO <why>: Windows Hello starting; foreground: ...

export interface PromptsAnalysis {
  /** "Windows Hello starting" lines: one per Hello check the app started. */
  starts: number;
  /** "Hello dialog appeared" lines (the watcher saw the dialog). */
  dialogs: number;
  dialogMs: number[];
  /** Checks the app withdrew because the caller (the provider) went away. */
  withdrawn: number;
  /** Dialogs still open 400 ms after a cancel. */
  left: number;
  lines: string[];
}

export function analysePrompts(text: string): PromptsAnalysis {
  const lines = text.split(/\r?\n/).filter(Boolean);
  const dialogs = lines.filter((l) => /Hello dialog appeared after (\d+) ms/.test(l));
  return {
    starts: lines.filter((l) => /Windows Hello starting/.test(l)).length,
    dialogs: dialogs.length,
    dialogMs: dialogs.map((l) => Number(/appeared after (\d+) ms/.exec(l)![1])),
    withdrawn: lines.filter((l) => /check cancelled by the caller/.test(l)).length,
    left: lines.filter((l) => /still open 400 ms after Cancel\(\): true/.test(l)).length,
    lines,
  };
}

/** The number of Hello checks the log shows: the starts (the dialog watcher can miss one, see the notes). */
export const helloCount = (p: PromptsAnalysis) => Math.max(p.starts, p.dialogs);

// ---------------------------------------------------------------------------------------------
// `lp-winauth-provider status` (read-only)

export interface ProviderStatus {
  raw: string;
  registered: boolean;
  enabled: boolean;
  paired: boolean;
  appReachable: boolean;
  /** "Unlocked" / "Locked", only while the app is reachable. */
  vault?: string;
  cacheRows?: number;
  /** "none (the real app)" or a development profile name. */
  profile?: string;
}

export function parseProviderStatus(raw: string): ProviderStatus {
  const get = (re: RegExp) => re.exec(raw)?.[1];
  const rows = get(/^credential cache: (\d+) row/m);
  return {
    raw,
    registered: /^registered: yes/m.test(raw),
    enabled: /^registered: yes, enabled/m.test(raw),
    paired: /^paired: yes/m.test(raw),
    appReachable: /^app reachable: yes/m.test(raw),
    vault: get(/^vault: (\w+)/m),
    cacheRows: rows === undefined ? undefined : Number(rows),
    profile: get(/^pipe profile: (.*)$/m)?.trim(),
  };
}

// ---------------------------------------------------------------------------------------------
// Judging one ceremony

export interface PhaseSpec {
  /** register | signin | cancel | locked | closed */
  id: string;
  action: "register" | "signin";
  /** "cancelled": the person cancels the Windows Hello prompt, the page must see NotAllowedError. */
  outcome: "success" | "cancelled";
}

export type WaitResult =
  | { kind: "event"; event: RpEvent }
  | { kind: "page-error"; pageError: RpEvent }
  | { kind: "timeout"; timeoutS: number; pageEvents: string[] };

export interface JudgeInput {
  phase: PhaseSpec;
  result: WaitResult;
  provider: ProviderAnalysis;
  hello: PromptsAnalysis;
  /** --virtual self-test: there is no provider and no app, so those checks are skipped. */
  virtual: boolean;
  /** The page's own timing from click to answer (includes the person's time). */
  pageMs?: number;
  phaseMs: number;
  /** Closed-app phase: is localpass.exe running after the ceremony? */
  appRunningAfter?: boolean;
}

export interface Judgement {
  rows: Row[];
  /** Extra help to print (not part of the table). */
  hints: string[];
  /** A registration (success phase) was verified by the relying party. */
  registered: boolean;
}

const describeChecks = (checks: RpCheck[]) => checks.map((c) => `${c.name}${c.detail ? ` (${c.detail})` : ""}`).join("; ");

export function judgePhase(input: JudgeInput): Judgement {
  const { phase, result, provider, hello, virtual } = input;
  const rows: Row[] = [];
  const hints: string[] = [];
  const name = (s: string) => `${phase.id}: ${s}`;
  const add = (status: Status, what: string, detail = "") => rows.push({ status, name: name(what), detail });
  let registered = false;

  if (result.kind === "timeout") {
    add("FAIL", "result", `nothing happened within ${result.timeoutS} s`);
    hints.push("Did the Windows sheet appear? Did you choose LocalPass in it? Was the LocalPass Windows Hello prompt answered (look for it in the taskbar)?");
    hints.push(`provider requests seen: ${provider.txs.length}; page events: ${JSON.stringify(result.pageEvents)}`);
    for (const l of provider.lines.slice(-5)) hints.push(`provider.log: ${l.text.slice(0, 120)}`);
    return { rows, hints, registered };
  }

  const verb = phase.action === "register" ? "registration" : "sign-in";
  if (phase.outcome === "success") {
    const label = `${verb} verified by the RP`;
    if (result.kind === "page-error") {
      add("FAIL", label, `the page got ${result.pageError.errorName}: ${result.pageError.errorMessage}`);
    } else {
      const e = result.event;
      const failed = (e.checks ?? []).filter((c) => !c.ok);
      if (e.verified && failed.length === 0) {
        const f = e.flags;
        add(
          "PASS",
          label,
          `${(e.checks ?? []).length} checks${f ? `; flags UP=${+f.up} UV=${+f.uv} BE=${+f.be} BS=${+f.bs}` : ""}; counter ${e.counter}${e.aaguid ? `; aaguid ${e.aaguid}` : ""}${e.fmt ? `; fmt ${e.fmt}` : ""}${e.userHandleMatches ? "; userHandle ok" : ""}`,
        );
        if (phase.action === "register") registered = true;
      } else {
        add("FAIL", label, describeChecks(failed) || e.error || "not verified");
      }
    }
  } else if (result.kind === "page-error") {
    const e = result.pageError;
    add(e.errorName === "NotAllowedError" ? "PASS" : "FAIL", "the page shows NotAllowedError", `${e.errorName}: ${(e.errorMessage ?? "").slice(0, 80)}`);
  } else {
    add("FAIL", "the page shows NotAllowedError", "the ceremony succeeded; was Hello not cancelled?");
  }

  if (virtual) {
    add("SKIP", "provider log and Hello checks", "no provider in --virtual mode");
    return { rows, hints, registered };
  }

  const txs = provider.txs;
  const tx = txs[0];
  const summary = txs.map((t) => `${t.op} ${t.hr ?? "no end"}`).join(", ");
  if (phase.outcome === "success") {
    const clean = txs.length > 0 && provider.nonZero.length === 0 && txs.every((t) => t.hr === "0x00000000");
    add(
      clean ? "PASS" : "FAIL",
      "provider log: every hr= is 0",
      txs.length === 0 ? "the provider saw no request (was LocalPass chosen in the sheet?)" : provider.nonZero.length ? provider.nonZero.join(" | ") : `${summary}${txs.some((t) => t.repeats) ? " (Windows repeated a call)" : ""}`,
    );
    if (phase.action === "signin" && txs.some((t) => t.hr === HR_NOT_FOUND)) {
      hints.push("NTE_NOT_FOUND on sign-in: the provider needs exactly one LocalPass passkey for localhost. Delete older test passkeys for localhost in the app and run again.");
    }
  } else {
    add(
      txs.some((t) => t.hr === HR_CANCELLED) ? "PASS" : "FAIL",
      `provider answered NTE_USER_CANCELLED (${HR_CANCELLED})`,
      txs.length === 0 ? "the provider saw no request (the sheet was closed before LocalPass was chosen?)" : summary,
    );
  }
  if (provider.cancelOps.length) add("INFO", "CancelOperation lines", provider.cancelOps.join(" | "));

  // One Hello check per ceremony. With the vault locked (or the app just started) the unlock may be
  // a master password instead, which leaves no Hello line: that is a warning, not a failure.
  const count = helloCount(hello);
  const mayUseMasterPassword = phase.id === "locked" || phase.id === "closed";
  const helloLabel = `app logged exactly 1 Hello prompt${phase.outcome === "cancelled" ? " (none after the cancel)" : ""}`;
  const helloDetail = `dialogs ${hello.dialogs}, starts ${hello.starts}${hello.withdrawn ? `, withdrawn ${hello.withdrawn}` : ""}${hello.left ? `, ${hello.left} dialog left open` : ""}`;
  if (count === 1) add("PASS", helloLabel, helloDetail);
  else if (count === 0 && mayUseMasterPassword) add("WARN", helloLabel, `${helloDetail}; no Hello logged (unlocked with the master password?)`);
  else add("FAIL", helloLabel, `${helloDetail}${count === 0 ? "; prompts.log shows none (app profile? an old build without the log?)" : "; more than one prompt for one ceremony"}`);

  const providerMs = tx?.finished !== undefined ? tx.finished - tx.first : undefined;
  add(
    "INFO",
    "timing",
    `page ${seconds(input.pageMs)} (includes your clicks), provider ${seconds(providerMs)}, RP verify ${result.kind === "event" ? (result.event.verifyMs ?? "?") : "?"} ms${hello.dialogMs[0] !== undefined ? `, Hello dialog up after ${hello.dialogMs[0]} ms` : ""}, phase ${seconds(input.phaseMs)}`,
  );

  if (phase.id === "closed") {
    add(provider.starting && provider.started ? "PASS" : "FAIL", "the provider started LocalPass", provider.started ? provider.started.slice(0, 110) : "no 'started ... (pid N)' line in provider.log");
    add(input.appRunningAfter ? "INFO" : "WARN", "LocalPass is running afterwards", input.appRunningAfter ? "left running (the provider started it); quit it from the tray if you do not want it" : "localpass.exe is not running after the sign-in");
  }
  return { rows, hints, registered };
}

// ---------------------------------------------------------------------------------------------
// The final table

export function formatTable(rows: Row[]): string[] {
  const w = Math.max(...rows.map((r) => r.name.length), 10);
  const lines = rows.map((r) => `${r.status.padEnd(4)}  ${r.name.padEnd(w)}  ${r.detail}`.trimEnd());
  const failures = rows.filter((r) => r.status === "FAIL").length;
  const warnings = rows.filter((r) => r.status === "WARN").length;
  lines.push("", failures ? `${failures} check(s) FAILED${warnings ? `, ${warnings} warning(s)` : ""}` : `all checks passed${warnings ? ` (${warnings} warning(s))` : ""}`);
  return lines;
}

export const failureCount = (rows: Row[]) => rows.filter((r) => r.status === "FAIL").length;
